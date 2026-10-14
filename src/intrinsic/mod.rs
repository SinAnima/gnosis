//! Intrinsic evidence collected beside the winning structural provider.
//!
//! Signature and EXIF facts do not compete in [`crate::ProviderRegistry::select`].
//! A missing field stays an explicit outcome. Conflicting timestamps stay as
//! separate facts; `creation_time` only lists them.

mod exif;
mod signature;

use crate::connectors::{ObjectDescriptor, ProtoData};
use crate::knowledge::{Diagnostic, DiagnosticSeverity};
use crate::status::UnderstandingStatus;
use crate::GnosisError;
pub use exif::ExifExtractor;
use serde::{Deserialize, Serialize};
pub use signature::SignatureExtractor;
use std::path::Path;

pub const SCHEMA_VERSION: u32 = 1;
pub const FACT_CAP: usize = 64;
const VALUE_CAP: usize = 1024;

/// Where a fact was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginStage {
    Proto,
    Intrinsic,
    Structural,
    Derived,
    Model,
}

/// Why a field is present or missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOutcome {
    Present,
    Absent,
    Unsupported,
    Unreadable,
    Truncated,
    Redacted,
    NotAttempted,
}

/// Typed fact value. Arrays are lists of strings so the record stays JSON-stable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FactValue {
    String(String),
    Integer(i64),
    Decimal(String),
    Bool(bool),
    Timestamp(String),
    Uri(String),
    Digest(String),
    Array(Vec<String>),
    Null,
}

/// How sure the extractor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    Observed,
    Derived,
    Inferred,
}

/// One namespaced fact, for example `exif:DateTimeOriginal`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetadataFact {
    pub key: String,
    pub origin: OriginStage,
    pub extractor_id: String,
    pub extractor_version: String,
    pub locator: String,
    pub value: FactValue,
    pub outcome: FieldOutcome,
    pub certainty: Certainty,
    pub sensitive: bool,
}

impl MetadataFact {
    pub fn observed(
        key: impl Into<String>,
        extractor_id: &str,
        extractor_version: &str,
        locator: impl Into<String>,
        value: FactValue,
    ) -> Self {
        let (value, outcome) = cap_value(value);
        Self {
            key: key.into(),
            origin: OriginStage::Intrinsic,
            extractor_id: extractor_id.to_string(),
            extractor_version: extractor_version.to_string(),
            locator: locator.into(),
            value,
            outcome,
            certainty: Certainty::Observed,
            sensitive: false,
        }
    }
}

/// Coverage of one compilation stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Partial,
    NotAttempted,
    Failed,
    Unreadable,
}

/// Bytes-versus-extension sniff. No external format crate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSignature {
    pub declared_media: String,
    pub sniffed: String,
    /// `spooled` when the stage saw an in-memory buffer rather than a seekable file.
    pub access: String,
}

/// Magic-byte kind plus the declared MIME type.
pub fn sniff(bytes: &[u8], declared_media: &str) -> FileSignature {
    let sniffed = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpeg"
    } else if bytes.starts_with(b"II*\x00") || bytes.starts_with(b"MM\x00*") {
        "tiff"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"%PDF") {
        "pdf"
    } else if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        "zip"
    } else {
        "unknown"
    };
    FileSignature {
        declared_media: declared_media.to_string(),
        sniffed: sniffed.to_string(),
        access: "spooled".to_string(),
    }
}

/// Facts from one stage, plus the stamp used to skip unchanged work.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntrinsicData {
    pub signature: FileSignature,
    pub facts: Vec<MetadataFact>,
    pub diagnostics: Vec<Diagnostic>,
    pub coverage: Coverage,
    pub extractor_stamp: String,
}

impl IntrinsicData {
    /// Every fact that contributes to creation time. Conflicts are kept.
    pub fn creation_time(&self) -> Vec<&MetadataFact> {
        self.facts
            .iter()
            .filter(|fact| fact.key == "exif:DateTimeOriginal" || fact.key == "fs:mtime")
            .collect()
    }
}

/// The five stage statuses, separate from the user-facing projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageStatus {
    pub acquisition: Coverage,
    pub intrinsic: Coverage,
    pub structural: Coverage,
    pub semantic: Coverage,
    pub review: String,
}

/// Persisted evidence for one object. Structural entities stay on the object row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactEvidenceRecord {
    pub schema_version: u32,
    pub proto: ProtoData,
    pub intrinsic: IntrinsicData,
    pub stages: StageStatus,
    /// Status returned by the structural reader, before the user-facing projection.
    pub structural_status: UnderstandingStatus,
    pub user_status: UnderstandingStatus,
    pub status_reason: Option<String>,
}

/// Bytes and descriptors an extractor may read. The buffer is the spooled copy.
pub struct ExtractContext<'a> {
    pub object: &'a ObjectDescriptor,
    pub proto: &'a ProtoData,
    pub bytes: &'a [u8],
    pub signature: &'a FileSignature,
}

pub trait IntrinsicExtractor: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> &str;
    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool;
    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>>;
}

/// Stable stamp of the registered extractors. A change re-runs only this stage.
pub fn extractor_stamp() -> String {
    let signature = SignatureExtractor;
    let exif = ExifExtractor;
    format!(
        "{}:{}+{}:{}",
        signature.id(),
        signature.version(),
        exif.id(),
        exif.version()
    )
}

pub fn extract_intrinsic(
    object: &ObjectDescriptor,
    proto: &ProtoData,
    bytes: &[u8],
) -> ArtifactEvidenceRecord {
    let mut signature = sniff(bytes, &object.media_type);
    signature.access = "spooled".to_string();
    let ctx = ExtractContext {
        object,
        proto,
        bytes,
        signature: &signature,
    };
    let extractors: [&dyn IntrinsicExtractor; 2] = [&SignatureExtractor, &ExifExtractor];
    let mut facts = acquisition_facts(proto);
    let mut diagnostics = Vec::new();
    let mut saw_domain = false;
    for extractor in extractors {
        if !extractor.probe(object, &signature) {
            continue;
        }
        match extractor.extract(&ctx) {
            Ok(found) => {
                if extractor.id() != "file-signature" && !found.is_empty() {
                    saw_domain = true;
                }
                for fact in found {
                    push_fact(&mut facts, fact);
                }
            }
            Err(err) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: format!("{}: {err}", extractor.id()),
                });
            }
        }
    }
    if facts.len() > FACT_CAP {
        facts.truncate(FACT_CAP);
        diagnostics.push(Diagnostic {
            severity: DiagnosticSeverity::Warning,
            message: format!("fact cap {FACT_CAP} reached"),
        });
    }
    let coverage = if diagnostics
        .iter()
        .any(|d| matches!(d.severity, DiagnosticSeverity::Error))
        && !saw_domain
    {
        Coverage::Unreadable
    } else if saw_domain {
        Coverage::Partial
    } else {
        Coverage::NotAttempted
    };
    let intrinsic = IntrinsicData {
        signature,
        facts,
        diagnostics,
        coverage,
        extractor_stamp: extractor_stamp(),
    };
    let stages = StageStatus {
        acquisition: Coverage::Complete,
        intrinsic: coverage,
        structural: Coverage::NotAttempted,
        semantic: Coverage::NotAttempted,
        review: "none".into(),
    };
    ArtifactEvidenceRecord {
        schema_version: SCHEMA_VERSION,
        proto: proto.clone(),
        intrinsic,
        stages,
        structural_status: UnderstandingStatus::Unknown,
        user_status: UnderstandingStatus::Unknown,
        status_reason: None,
    }
}

/// User-facing status. EXIF metadata is partial: it is not scene understanding.
pub fn project_user_status(
    provider_id: &str,
    structural: UnderstandingStatus,
    reason: Option<String>,
) -> (UnderstandingStatus, Option<String>) {
    if provider_id == "exif" && structural == UnderstandingStatus::Understood {
        (
            UnderstandingStatus::PartiallyUnderstood,
            Some("exif metadata; semantic understanding not attempted".into()),
        )
    } else {
        (structural, reason)
    }
}

pub fn apply_structural(
    record: &mut ArtifactEvidenceRecord,
    provider_id: &str,
    structural: UnderstandingStatus,
    reason: Option<String>,
) {
    record.stages.structural = match structural {
        UnderstandingStatus::Understood => Coverage::Complete,
        UnderstandingStatus::PartiallyUnderstood => Coverage::Partial,
        UnderstandingStatus::Failed => Coverage::Failed,
        UnderstandingStatus::Unknown => Coverage::NotAttempted,
    };
    record.stages.semantic = if provider_id == "llm-semantic" {
        Coverage::Partial
    } else {
        Coverage::NotAttempted
    };
    record.structural_status = structural;
    let (status, reason) = project_user_status(provider_id, structural, reason);
    record.user_status = status;
    record.status_reason = reason;
}

/// Same identity only when extractor id, version, and locator match.
fn push_fact(facts: &mut Vec<MetadataFact>, fact: MetadataFact) {
    if facts.iter().any(|existing| {
        existing.extractor_id == fact.extractor_id
            && existing.extractor_version == fact.extractor_version
            && existing.locator == fact.locator
            && existing.key == fact.key
    }) {
        return;
    }
    facts.push(fact);
}

fn acquisition_facts(proto: &ProtoData) -> Vec<MetadataFact> {
    let mut facts = Vec::new();
    if let Some(modified) = proto.modified {
        let stamp = chrono::DateTime::<chrono::Utc>::from(modified);
        facts.push(MetadataFact {
            key: "fs:mtime".into(),
            origin: OriginStage::Proto,
            extractor_id: "acquisition".into(),
            extractor_version: "0.1.0".into(),
            locator: "filesystem:mtime".into(),
            value: FactValue::Timestamp(stamp.to_rfc3339()),
            outcome: FieldOutcome::Present,
            certainty: Certainty::Observed,
            sensitive: false,
        });
    }
    facts
}

fn cap_value(value: FactValue) -> (FactValue, FieldOutcome) {
    match value {
        FactValue::String(text) if text.chars().count() > VALUE_CAP => (
            FactValue::String(text.chars().take(VALUE_CAP).collect()),
            FieldOutcome::Truncated,
        ),
        other => (other, FieldOutcome::Present),
    }
}

/// Live inspect of one local file: proto, sniff, intrinsic, then the structural reader.
pub fn inspect_file(path: &Path) -> crate::Result<InspectReport> {
    let bytes = std::fs::read(path)
        .map_err(|err| GnosisError::Pipeline(format!("read {}: {err}", path.display())))?;
    let relative = path.to_path_buf();
    let object = crate::ObjectDescriptor {
        id: crate::ObjectId::from_path(Path::new("."), &relative),
        path: path.to_path_buf(),
        relative_path: relative,
        media_type: mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_string(),
        size: bytes.len() as u64,
        extension: path
            .extension()
            .map(|ext| ext.to_string_lossy().to_string()),
        modified: std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok()),
        is_dir: false,
        etag: None,
    };
    let proto = ProtoData {
        connector: "filesystem".into(),
        path: object.path.clone(),
        relative_path: object.relative_path.clone(),
        filename: object
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        extension: object.extension.clone(),
        media_type: object.media_type.clone(),
        size: object.size,
        modified: object.modified,
        fingerprint: Some(crate::connectors::filesystem::fingerprint_bytes(&bytes)),
        ..ProtoData::default()
    };
    let mut evidence = extract_intrinsic(&object, &proto, &bytes);
    let registry = crate::providers::default_registry();
    let structural = if let Some((provider, _)) = registry.select(&object, &proto) {
        let mut reader = crate::BytesContentReader::new(bytes);
        match provider.analyze(&object, &proto, &mut reader) {
            Ok(result) => {
                apply_structural(
                    &mut evidence,
                    provider.id().as_str(),
                    result.status,
                    result.classification_reason.clone(),
                );
                Some(StructuralView {
                    provider: provider.id().as_str().to_string(),
                    status: result.status,
                    entities: result
                        .record
                        .entities
                        .into_iter()
                        .map(|entity| InspectEntity {
                            kind: entity.kind,
                            name: entity.name,
                        })
                        .collect(),
                })
            }
            Err(err) => {
                apply_structural(
                    &mut evidence,
                    provider.id().as_str(),
                    UnderstandingStatus::Failed,
                    Some(err.to_string()),
                );
                Some(StructuralView {
                    provider: provider.id().as_str().to_string(),
                    status: UnderstandingStatus::Failed,
                    entities: Vec::new(),
                })
            }
        }
    } else {
        None
    };
    Ok(InspectReport {
        evidence,
        structural,
        semantic: "not_attempted".into(),
    })
}

#[derive(Clone, Debug, Serialize)]
pub struct InspectEntity {
    pub kind: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructuralView {
    pub provider: String,
    pub status: UnderstandingStatus,
    pub entities: Vec<InspectEntity>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InspectReport {
    pub evidence: ArtifactEvidenceRecord,
    pub structural: Option<StructuralView>,
    pub semantic: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObjectId;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn sample_object() -> ObjectDescriptor {
        ObjectDescriptor {
            id: ObjectId::new("obj:camera.jpg"),
            path: PathBuf::from("camera.jpg"),
            relative_path: PathBuf::from("camera.jpg"),
            is_dir: false,
            size: 3,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_714_521_600)),
            media_type: "image/jpeg".into(),
            extension: Some("jpg".into()),
            etag: None,
        }
    }

    struct Boom;

    impl IntrinsicExtractor for Boom {
        fn id(&self) -> &str {
            "boom"
        }
        fn version(&self) -> &str {
            "0.0.1"
        }
        fn probe(&self, _: &ObjectDescriptor, _: &FileSignature) -> bool {
            true
        }
        fn extract(&self, _: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
            Err(GnosisError::provider("boom", "unreadable header"))
        }
    }

    #[test]
    fn sniff_known_signatures() {
        assert_eq!(
            sniff(&[0xFF, 0xD8, 0xFF, 0xE1], "image/jpeg").sniffed,
            "jpeg"
        );
        assert_eq!(sniff(b"II*\x00rest", "image/tiff").sniffed, "tiff");
        assert_eq!(sniff(b"%PDF-1.4", "application/pdf").sniffed, "pdf");
        assert_eq!(sniff(b"PK\x03\x04", "application/zip").sniffed, "zip");
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n", "image/png").sniffed, "png");
    }

    #[test]
    fn extractor_error_keeps_proto_and_dedups_identical_locator() {
        let object = sample_object();
        let proto = ProtoData {
            filename: "camera.jpg".into(),
            modified: object.modified,
            ..ProtoData::default()
        };
        let signature = sniff(&[0xFF, 0xD8, 0xFF], &object.media_type);
        let ctx = ExtractContext {
            object: &object,
            proto: &proto,
            bytes: &[0xFF, 0xD8, 0xFF],
            signature: &signature,
        };
        let err = Boom.extract(&ctx).unwrap_err();
        assert!(err.to_string().contains("unreadable"));
        let record = extract_intrinsic(&object, &proto, &[0xFF, 0xD8, 0xFF]);
        assert_eq!(record.proto.filename, "camera.jpg");
        assert!(record
            .intrinsic
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("exif")));
        assert!(record
            .intrinsic
            .facts
            .iter()
            .any(|fact| fact.key == "fs:mtime"));
        let mut facts = Vec::new();
        let fact = MetadataFact::observed(
            "exif:DateTimeOriginal",
            "exif",
            "0.1.0",
            "ifd:0",
            FactValue::Timestamp("2020-01-02 03:04:05".into()),
        );
        push_fact(&mut facts, fact.clone());
        push_fact(&mut facts, fact);
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn unicode_value_and_fact_cap() {
        let fact = MetadataFact::observed(
            "exif:ImageDescription",
            "exif",
            "0.1.0",
            "ifd:0",
            FactValue::String("写真 — café".into()),
        );
        assert_eq!(fact.outcome, FieldOutcome::Present);
        match fact.value {
            FactValue::String(text) => assert!(text.contains('絵') || text.contains('写')),
            _ => panic!("string"),
        }
        let long = "ü".repeat(VALUE_CAP + 8);
        let truncated = MetadataFact::observed(
            "exif:UserComment",
            "exif",
            "0.1.0",
            "ifd:1",
            FactValue::String(long),
        );
        assert_eq!(truncated.outcome, FieldOutcome::Truncated);
        let mut facts = Vec::new();
        for index in 0..(FACT_CAP + 3) {
            push_fact(
                &mut facts,
                MetadataFact::observed(
                    format!("exif:Tag{index}"),
                    "exif",
                    "0.1.0",
                    format!("ifd:{index}"),
                    FactValue::Integer(index as i64),
                ),
            );
        }
        assert!(facts.len() > FACT_CAP);
        facts.truncate(FACT_CAP);
        assert_eq!(facts.len(), FACT_CAP);
    }

    #[test]
    fn creation_time_keeps_both_timestamps() {
        let object = sample_object();
        let proto = ProtoData {
            modified: object.modified,
            ..ProtoData::default()
        };
        let mut record = extract_intrinsic(&object, &proto, b"not-an-image");
        record.intrinsic.facts.push(MetadataFact::observed(
            "exif:DateTimeOriginal",
            "exif",
            "0.1.0",
            "ifd:DateTimeOriginal",
            FactValue::Timestamp("2020-01-02 03:04:05".into()),
        ));
        let sources = record.intrinsic.creation_time();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].key, "fs:mtime");
        assert_eq!(sources[1].key, "exif:DateTimeOriginal");
    }

    #[test]
    fn camera_jpeg_keeps_signature_exif_and_mtime_in_order() {
        let bytes = std::fs::read("fixtures/enterprise-mixed/images/camera.jpg")
            .expect("camera.jpg fixture");
        let object = sample_object();
        let proto = ProtoData {
            filename: "camera.jpg".into(),
            modified: object.modified,
            ..ProtoData::default()
        };
        let record = extract_intrinsic(&object, &proto, &bytes);
        let keys: Vec<&str> = record
            .intrinsic
            .facts
            .iter()
            .map(|fact| fact.key.as_str())
            .collect();
        assert_eq!(keys[0], "fs:mtime");
        assert!(keys.contains(&"file:declared_media"));
        assert!(keys.contains(&"file:sniffed"));
        assert!(keys.contains(&"exif:DateTimeOriginal"));
        let sniffed = keys.iter().position(|key| *key == "file:sniffed").unwrap();
        let exif = keys
            .iter()
            .position(|key| *key == "exif:DateTimeOriginal")
            .unwrap();
        assert!(sniffed < exif);
        let times = record.intrinsic.creation_time();
        assert_eq!(times.len(), 2);
        assert_eq!(record.intrinsic.signature.sniffed, "jpeg");
        assert_eq!(record.intrinsic.signature.access, "spooled");
        assert_eq!(record.stages.semantic, Coverage::NotAttempted);
    }
}
