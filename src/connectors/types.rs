use crate::ids::ObjectId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
