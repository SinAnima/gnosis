use crate::ids::ObjectId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Keys and values kept on S3 user metadata and tags.
pub const S3_MAP_CAP: usize = 32;
pub const S3_VALUE_CAP: usize = 256;

/// Descriptor for a discovered digital object (not necessarily a local file forever).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectDescriptor {
    pub id: ObjectId,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub media_type: String,
    pub extension: Option<String>,
    /// S3 ETag when the object was listed. Filesystem scans leave this empty.
    #[serde(default)]
    pub etag: Option<String>,
}

/// Context collected before content analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProtoData {
    pub connector: String,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub filename: String,
    pub extension: Option<String>,
    pub parent_path: Option<PathBuf>,
    pub neighbor_names: Vec<String>,
    pub media_type: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub permissions: Option<String>,
    pub fingerprint: Option<String>,
    pub git: Option<GitProtoData>,
    /// Set for web pages. Absent on filesystem and S3 objects.
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub requested_url: Option<String>,
    #[serde(default)]
    pub final_url: Option<String>,
    /// Listing and opt-in inspect facts for an object store key.
    #[serde(default)]
    pub s3: Option<S3ProtoData>,
    /// Relation snapshot for a catalog scan. Absent on files, web pages, and S3 objects.
    #[serde(default)]
    pub catalog: Option<CatalogProtoData>,
}

/// Object-store facts copied onto [`ProtoData`]. A multipart ETag is not a digest.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct S3ProtoData {
    #[serde(default)]
    pub storage_class: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
    /// `single` or `multipart`.
    #[serde(default)]
    pub etag_kind: String,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub user_metadata: BTreeMap<String, String>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

/// One column of a catalog relation. A comment is stored as written.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub ordinal: i32,
    #[serde(default)]
    pub comment: Option<String>,
}

/// A declared foreign key. `target_discovered` is set when that relation is in this scan.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogForeignKey {
    pub columns: Vec<String>,
    pub target_schema: String,
    pub target_relation: String,
    pub target_columns: Vec<String>,
    #[serde(default)]
    pub target_discovered: bool,
}

/// Catalog facts for one relation. This is not a copy of table rows.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogProtoData {
    pub database: String,
    pub schema: String,
    pub relation: String,
    /// `table`, `view`, or `materialized_view`.
    pub kind: String,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub view_definition: Option<String>,
    #[serde(default)]
    pub columns: Vec<CatalogColumn>,
    #[serde(default)]
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub unique_keys: Vec<Vec<String>>,
    #[serde(default)]
    pub indexes: Vec<String>,
    #[serde(default)]
    pub foreign_keys: Vec<CatalogForeignKey>,
    /// View dependencies whose schema was not allowlisted. Not edges.
    #[serde(default)]
    pub unresolved: Vec<String>,
}

/// `multipart` when the ETag contains `-`. The value is not a content hash.
pub fn etag_kind(etag: &str) -> &'static str {
    let bare = etag.trim_matches('"');
    if bare.contains('-') {
        "multipart"
    } else {
        "single"
    }
}

/// Keep the first 32 keys after sorting, and trim each key and value to 256 characters.
pub fn cap_s3_map(entries: impl IntoIterator<Item = (String, String)>) -> BTreeMap<String, String> {
    let mut pairs: Vec<(String, String)> = entries.into_iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = BTreeMap::new();
    for (key, value) in pairs {
        if out.len() >= S3_MAP_CAP {
            break;
        }
        let key: String = key.chars().take(S3_VALUE_CAP).collect();
        if key.is_empty() {
            continue;
        }
        let value: String = value.chars().take(S3_VALUE_CAP).collect();
        out.insert(key, value);
    }
    out
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GitProtoData {
    pub repository_root: PathBuf,
    pub branch: Option<String>,
    pub tracked: Option<bool>,
    pub last_commit_id: Option<String>,
    pub last_commit_author: Option<String>,
    pub last_commit_time: Option<String>,
    pub last_commit_summary: Option<String>,
}

impl ObjectDescriptor {
    pub fn from_path(
        root: &Path,
        path: &Path,
        is_dir: bool,
        size: u64,
        modified: Option<SystemTime>,
    ) -> Self {
        let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        let extension = path.extension().map(|e| e.to_string_lossy().to_string());
        let media_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        Self {
            id: ObjectId::from_path(root, path),
            path: path.to_path_buf(),
            relative_path,
            is_dir,
            size,
            modified,
            media_type,
            extension,
            etag: None,
        }
    }
}
