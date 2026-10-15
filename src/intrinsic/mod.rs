//! Intrinsic evidence collected beside the winning structural provider.
//!
//! Signature, EXIF, and container-metadata facts do not compete in [`crate::ProviderRegistry::select`].
//! A missing field stays an explicit outcome. Conflicting timestamps stay as
//! separate facts; `creation_time` only lists them.

mod avif;
mod exif;
mod png;
mod psd;
mod signature;
mod webp;

use crate::connectors::{ObjectDescriptor, ProtoData};
use crate::knowledge::{Diagnostic, DiagnosticSeverity};
use crate::status::UnderstandingStatus;
use crate::GnosisError;
pub use avif::AvifExtractor;
pub use exif::ExifExtractor;
pub use png::PngExtractor;
pub use psd::PsdExtractor;
use serde::{Deserialize, Serialize};
pub use signature::SignatureExtractor;
use std::path::Path;
pub use webp::WebpExtractor;

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
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"8BPS") {
        "psd"
    } else if is_avif(bytes) {
        "avif"
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

/// `ftyp` major brand `avif` or `avis`.
fn is_avif(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && (bytes[8..12] == *b"avif" || bytes[8..12] == *b"avis")
}

fn registered_extractors() -> [&'static dyn IntrinsicExtractor; 6] {
    [
        &SignatureExtractor,
        &ExifExtractor,
        &PngExtractor,
        &WebpExtractor,
        &AvifExtractor,
        &PsdExtractor,
    ]
}

/// Stable stamp of the registered extractors. A change re-runs only this stage.
pub fn extractor_stamp() -> String {
    registered_extractors()
        .iter()
        .map(|extractor| format!("{}:{}", extractor.id(), extractor.version()))
        .collect::<Vec<_>>()
        .join("+")
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
    let extractors = registered_extractors();
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
    let (mut status, mut reason) = project_user_status(provider_id, structural, reason);
    if status == UnderstandingStatus::Unknown && record.intrinsic.coverage == Coverage::Partial {
        status = UnderstandingStatus::PartiallyUnderstood;
        reason = Some("container metadata; semantic understanding not attempted".into());
    }
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

    fn fact<'a>(record: &'a ArtifactEvidenceRecord, key: &str) -> &'a MetadataFact {
        record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == key)
            .unwrap_or_else(|| panic!("missing {key}"))
    }

    #[test]
    fn sniff_webp_avif_and_psd() {
        assert_eq!(
            sniff(b"RIFF\x00\x00\x00\x00WEBP", "image/webp").sniffed,
            "webp"
        );
        assert_eq!(
            sniff(b"\x00\x00\x00\x18ftypavif", "image/avif").sniffed,
            "avif"
        );
        assert_eq!(
            sniff(b"\x00\x00\x00\x18ftypavis", "image/avif").sniffed,
            "avif"
        );
        assert_eq!(
            sniff(b"8BPS\x00\x01", "image/vnd.adobe.photoshop").sniffed,
            "psd"
        );
    }

    #[test]
    fn png_text_and_ihdr_project_partial() {
        let bytes = std::fs::read("fixtures/enterprise-mixed/images/caption.png").unwrap();
        let object = sample_object();
        let proto = ProtoData {
            filename: "caption.png".into(),
            ..ProtoData::default()
        };
        let mut record = extract_intrinsic(&object, &proto, &bytes);
        assert_eq!(record.intrinsic.signature.sniffed, "png");
        assert_eq!(fact(&record, "png:width").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "png:height").value, FactValue::Integer(1));
        assert_eq!(
            fact(&record, "png:text:Description").value,
            FactValue::String("Synthetic caption".into())
        );
        assert!(!record
            .intrinsic
            .facts
            .iter()
            .any(|item| item.key.contains("icc")));
        apply_structural(
            &mut record,
            "generic-metadata",
            UnderstandingStatus::Unknown,
            None,
        );
        assert_eq!(record.user_status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(
            record.status_reason.as_deref(),
            Some("container metadata; semantic understanding not attempted")
        );
        assert_eq!(record.stages.semantic, Coverage::NotAttempted);
        assert_eq!(record.proto.filename, "caption.png");
    }

    #[test]
    fn truncated_and_huge_png_keep_proto() {
        let object = sample_object();
        let proto = ProtoData {
            filename: "broken.png".into(),
            ..ProtoData::default()
        };
        let truncated = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00";
        let record = extract_intrinsic(&object, &proto, truncated);
        assert_eq!(record.proto.filename, "broken.png");
        assert!(record
            .intrinsic
            .diagnostics
            .iter()
            .any(|diag| diag.message.starts_with("png:")));
        let mut huge = b"\x89PNG\r\n\x1a\n".to_vec();
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        huge.extend_from_slice(b"tEXt");
        let record = extract_intrinsic(&object, &proto, &huge);
        assert_eq!(record.proto.filename, "broken.png");
        assert_eq!(record.intrinsic.coverage, Coverage::Unreadable);
    }

    #[test]
    fn hostile_compressed_png_text_keeps_proto() {
        let object = sample_object();
        let proto = ProtoData {
            filename: "hostile.png".into(),
            ..ProtoData::default()
        };
        let ihdr = {
            let mut data = 1u32.to_be_bytes().to_vec();
            data.extend_from_slice(&1u32.to_be_bytes());
            data.extend_from_slice(&[8, 2, 0, 0, 0]);
            png_chunk(b"IHDR", &data)
        };
        let ztxt = png_chunk(b"zTXt", b"Comment\x00\x00not-deflate");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&ihdr);
        bytes.extend_from_slice(&ztxt);
        let record = extract_intrinsic(&object, &proto, &bytes);
        assert_eq!(record.proto.filename, "hostile.png");
        assert!(record
            .intrinsic
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("zTXt") || diag.message.starts_with("png:")));
    }

    fn png_chunk(tag: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let crc = crc32(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    #[test]
    fn webp_keeps_dimensions_exif_and_xmp_presence() {
        let bytes = std::fs::read("fixtures/enterprise-mixed/images/photo.webp").unwrap();
        let mut object = sample_object();
        object.extension = Some("webp".into());
        object.media_type = "image/webp".into();
        let proto = ProtoData {
            filename: "photo.webp".into(),
            modified: object.modified,
            ..ProtoData::default()
        };
        let record = extract_intrinsic(&object, &proto, &bytes);
        assert_eq!(fact(&record, "webp:width").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "webp:height").value, FactValue::Integer(1));
        assert!(matches!(
            fact(&record, "exif:DateTimeOriginal").value,
            FactValue::Timestamp(_)
        ));
        assert_eq!(record.intrinsic.creation_time().len(), 2);
        assert_eq!(
            fact(&record, "webp:xmp_present").value,
            FactValue::Bool(true)
        );
        assert!(fact(&record, "webp:xmp_length").value != FactValue::Integer(0));
        assert!(record.intrinsic.facts.iter().all(|item| {
            item.key != "dc:title"
                && !matches!(&item.value, FactValue::String(text) if text.contains("dc:title"))
        }));
        assert_eq!(record.stages.semantic, Coverage::NotAttempted);
    }

    #[test]
    fn avif_dimensions_do_not_store_the_payload() {
        let bytes = std::fs::read("fixtures/enterprise-mixed/images/still.avif").unwrap();
        let object = sample_object();
        let proto = ProtoData {
            filename: "still.avif".into(),
            ..ProtoData::default()
        };
        let record = extract_intrinsic(&object, &proto, &bytes);
        assert_eq!(record.intrinsic.signature.sniffed, "avif");
        assert_eq!(fact(&record, "avif:width").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "avif:height").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "avif:bit_depth").value, FactValue::Integer(8));
        assert_eq!(
            fact(&record, "avif:still_picture").value,
            FactValue::Bool(true)
        );
        assert!(record.intrinsic.facts.iter().all(|item| {
            !item.key.contains("payload")
                && !matches!(&item.value, FactValue::String(text) if text.len() > 64)
        }));
        assert_eq!(record.stages.semantic, Coverage::NotAttempted);
    }

    #[test]
    fn psd_header_and_layer_name() {
        let bytes = std::fs::read("fixtures/enterprise-mixed/images/art.psd").unwrap();
        let object = sample_object();
        let proto = ProtoData {
            filename: "art.psd".into(),
            ..ProtoData::default()
        };
        let record = extract_intrinsic(&object, &proto, &bytes);
        assert_eq!(record.intrinsic.signature.sniffed, "psd");
        assert_eq!(fact(&record, "psd:width").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "psd:height").value, FactValue::Integer(1));
        assert_eq!(fact(&record, "psd:depth").value, FactValue::Integer(8));
        assert_eq!(
            fact(&record, "psd:color_mode").value,
            FactValue::String("grayscale".into())
        );
        assert_eq!(
            fact(&record, "psd:layers").value,
            FactValue::Array(vec!["Ink".into()])
        );
    }
}
