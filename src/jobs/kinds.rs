//! Built-in job kinds and their argument / result shapes.

use crate::connectors::types::ObjectDescriptor;
use crate::ids::ObjectId;
use crate::status::UnderstandingStatus;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Analyze one discovered artifact (filesystem or S3 object).
pub const KIND_ANALYZE_OBJECT: &str = "analyze_object";

/// Serializable job arguments for [`KIND_ANALYZE_OBJECT`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyzeObjectArgs {
    pub id: String,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// Unix milliseconds since epoch, if known.
    pub modified_unix_ms: Option<u64>,
    pub media_type: String,
    pub extension: Option<String>,
    /// S3 ETag, when the descriptor had one.
    #[serde(default)]
    pub etag: Option<String>,
}

impl AnalyzeObjectArgs {
    pub fn from_descriptor(d: &ObjectDescriptor) -> Self {
        Self {
            id: d.id.as_str().to_string(),
            path: d.path.clone(),
            relative_path: d.relative_path.clone(),
            is_dir: d.is_dir,
            size: d.size,
            modified_unix_ms: d.modified.and_then(system_time_to_unix_ms),
            media_type: d.media_type.clone(),
            extension: d.extension.clone(),
            etag: d.etag.clone(),
        }
    }

    pub fn to_descriptor(&self) -> ObjectDescriptor {
        ObjectDescriptor {
            id: ObjectId::new(self.id.clone()),
            path: self.path.clone(),
            relative_path: self.relative_path.clone(),
            is_dir: self.is_dir,
            size: self.size,
            modified: self.modified_unix_ms.and_then(unix_ms_to_system_time),
            media_type: self.media_type.clone(),
            extension: self.extension.clone(),
            etag: self.etag.clone(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }
}

/// Result payload stored when an analyze job completes successfully.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalyzeObjectResult {
    pub object_id: String,
    pub status: UnderstandingStatus,
    pub provider: Option<String>,
    pub entities: usize,
    pub relationships: usize,
    pub classification_reason: Option<String>,
}

fn system_time_to_unix_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn unix_ms_to_system_time(ms: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_millis(ms))
}
