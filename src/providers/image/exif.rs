//! Deterministic EXIF for JPEG and TIFF.
//!
//! Reads the primary IFD, the extended Exif IFD, the GPS IFD, and the
//! interoperability IFD. Maker notes, thumbnails, and other opaque blobs
//! are left out.

use crate::providers::util::{attrs, entity, ext_is};
use crate::{
    AnalysisResult, Confidence, ContentReader, Diagnostic, DiagnosticSeverity, KnowledgeRecord,
    ObjectDescriptor, ProtoData, ProviderId, Result, Support, UnderstandingProvider,
    UnderstandingStatus,
};
use std::io::Cursor;

const MAX_TAGS: usize = 64;

const EXTENSIONS: &[&str] = &["jpg", "jpeg", "tif", "tiff"];

pub struct ExifProvider;

impl UnderstandingProvider for ExifProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("exif")
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        let media = object.media_type.split(';').next().unwrap_or("").trim();
        if ext_is(&object.extension, EXTENSIONS)
            || media.eq_ignore_ascii_case("image/jpeg")
            || media.eq_ignore_ascii_case("image/tiff")
        {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let mut buf = Vec::new();
        content.read_to_end(&mut buf)?;
        let provider = self.id();
        let path = &object.relative_path;
        let name = if proto.filename.is_empty() {
            path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "image".into())
        } else {
            proto.filename.clone()
        };

        let mut diagnostics = Vec::new();
        let mut attributes = attrs(&[("format", "exif")]);
        let mut understood = false;

        match read_exif(&buf) {
            Ok(ReadExif::Complete(exif)) => {
                fill_attributes(&exif, &mut attributes);
                understood = true;
            }
            Ok(ReadExif::Partial { exif, errors }) => {
                fill_attributes(&exif, &mut attributes);
                for message in errors {
                    diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Warning,
                        message: format!("truncated or invalid EXIF: {message}"),
                    });
                }
            }
            Err(ExifRead::Missing) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Info,
                    message: "image has no EXIF segment".into(),
                });
            }
            Err(ExifRead::Broken(message)) => {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Warning,
                    message: format!("truncated or invalid EXIF: {message}"),
                });
            }
        }

        let status = if understood {
            UnderstandingStatus::Understood
        } else {
            UnderstandingStatus::PartiallyUnderstood
        };
        let reason = if understood {
            "exif metadata"
        } else {
            "image without usable exif"
        };
        let image = entity(
            "image",
            &name,
            path,
            &object.id,
            &provider,
            1,
            attributes,
            if understood {
                Confidence::High
            } else {
                Confidence::Low
            },
        );
        Ok(AnalysisResult {
            record: KnowledgeRecord {
                entities: vec![image],
                relationships: Vec::new(),
                diagnostics,
                status: Some(status),
                classification_reason: Some(reason.into()),
            },
            status,
            classification_reason: Some(reason.into()),
        })
    }
}

enum ExifRead {
    Missing,
    Broken(String),
}

enum ReadExif {
    Complete(exif::Exif),
    Partial {
        exif: exif::Exif,
        errors: Vec<String>,
    },
}

fn read_exif(bytes: &[u8]) -> std::result::Result<ReadExif, ExifRead> {
    let mut cursor = Cursor::new(bytes);
    match exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut cursor)
    {
        Ok(exif) => Ok(ReadExif::Complete(exif)),
        Err(error) => {
            let mut errors = Vec::new();
            match error.distill_partial_result(|found| {
                errors.extend(found.iter().map(|error| error.to_string()));
            }) {
                Ok(exif) => Ok(ReadExif::Partial { exif, errors }),
                Err(exif::Error::NotFound(_)) => Err(ExifRead::Missing),
                Err(error) => Err(ExifRead::Broken(error.to_string())),
            }
        }
    }
}

fn fill_attributes(exif: &exif::Exif, attributes: &mut crate::AttributeMap) {
    if let Some(value) = text_field(exif, exif::Tag::Make, exif::In::PRIMARY) {
        attributes.insert("make".into(), value);
    }
    if let Some(value) = text_field(exif, exif::Tag::Model, exif::In::PRIMARY) {
        attributes.insert("model".into(), value);
    }
    if let Some(value) = text_field(exif, exif::Tag::Orientation, exif::In::PRIMARY) {
        attributes.insert("orientation".into(), value);
    }
    if let Some(value) = text_field(exif, exif::Tag::DateTimeOriginal, exif::In::PRIMARY) {
        attributes.insert("datetime_original".into(), value);
    }
    if let Some(value) = gps_decimal(exif, true) {
        attributes.insert("gps_latitude".into(), value);
    }
    if let Some(value) = gps_decimal(exif, false) {
        attributes.insert("gps_longitude".into(), value);
    }
    if let Some(value) = gps_altitude(exif) {
        attributes.insert("gps_altitude".into(), value);
    }

    let mut stored = 0usize;
    for field in exif.fields() {
        if stored >= MAX_TAGS {
            break;
        }
        if field.ifd_num == exif::In::THUMBNAIL || skip_field(field) {
            continue;
        }
        let key = format!("exif.{}", field.tag);
        if attributes.contains_key(&key) || is_well_known(field.tag) {
            continue;
        }
        let value = display_text(field);
        if value.is_empty() {
            continue;
        }
        attributes.insert(key, value);
        stored += 1;
    }
}

fn is_well_known(tag: exif::Tag) -> bool {
    matches!(
        tag,
        exif::Tag::Make
            | exif::Tag::Model
            | exif::Tag::Orientation
            | exif::Tag::DateTimeOriginal
            | exif::Tag::GPSLatitude
            | exif::Tag::GPSLatitudeRef
            | exif::Tag::GPSLongitude
            | exif::Tag::GPSLongitudeRef
            | exif::Tag::GPSAltitude
            | exif::Tag::GPSAltitudeRef
    )
}

fn skip_field(field: &exif::Field) -> bool {
    matches!(
        field.tag,
        exif::Tag::MakerNote
            | exif::Tag::JPEGInterchangeFormat
            | exif::Tag::JPEGInterchangeFormatLength
    ) || matches!(field.value, exif::Value::Undefined(_, _))
        || matches!(field.value, exif::Value::Byte(ref bytes) if bytes.len() > 16)
}

fn text_field(exif: &exif::Exif, tag: exif::Tag, ifd: exif::In) -> Option<String> {
    let field = exif.get_field(tag, ifd)?;
    if skip_field(field) {
        return None;
    }
    let text = display_text(field);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn gps_decimal(exif: &exif::Exif, latitude: bool) -> Option<String> {
    let (coord, reference_tag) = if latitude {
        (exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef)
    } else {
        (exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef)
    };
    let parts = {
        let field = field_in(exif, coord)?;
        rationals(&field.value)?
    };
    let reference = field_in(exif, reference_tag)
        .map(display_text)
        .unwrap_or_default();
    let degrees = *parts.first()?;
    let minutes = parts.get(1).copied().unwrap_or(0.0);
    let seconds = parts.get(2).copied().unwrap_or(0.0);
    let mut decimal = degrees + minutes / 60.0 + seconds / 3600.0;
    if reference.starts_with('S') || reference.starts_with('W') {
        decimal = -decimal;
    }
    Some(format!("{decimal:.6}"))
}

fn gps_altitude(exif: &exif::Exif) -> Option<String> {
    let mut meters = {
        let field = field_in(exif, exif::Tag::GPSAltitude)?;
        *rationals(&field.value)?.first()?
    };
    let below = field_in(exif, exif::Tag::GPSAltitudeRef)
        .and_then(|field| field.value.get_uint(0))
        .is_some_and(|value| value == 1);
    if below {
        meters = -meters;
    }
    Some(format!("{meters:.3}"))
}

fn field_in(exif: &exif::Exif, tag: exif::Tag) -> Option<&exif::Field> {
    exif.get_field(tag, exif::In::PRIMARY)
}

fn display_text(field: &exif::Field) -> String {
    field
        .display_value()
        .to_string()
        .trim()
        .trim_matches('"')
        .to_string()
}

fn rationals(value: &exif::Value) -> Option<Vec<f64>> {
    match value {
        exif::Value::Rational(values) => Some(values.iter().map(exif::Rational::to_f64).collect()),
        exif::Value::SRational(values) => {
            Some(values.iter().map(exif::SRational::to_f64).collect())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::ExifProvider;
    use crate::content::BytesContentReader;
    use crate::providers::default_registry;
    use crate::{
        ObjectDescriptor, ObjectId, ProtoData, Support, UnderstandingProvider, UnderstandingStatus,
    };
    use std::path::PathBuf;

    fn image(name: &str, media: &str) -> (ObjectDescriptor, ProtoData) {
        let path = PathBuf::from(name);
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().into_owned());
        let object = ObjectDescriptor {
            id: ObjectId::new(format!("obj:{name}")),
            path: path.clone(),
            relative_path: path.clone(),
            is_dir: false,
            size: 32,
            modified: None,
            media_type: media.into(),
            extension: extension.clone(),
            etag: None,
        };
        let proto = ProtoData {
            filename: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.clone(),
            relative_path: path,
            extension,
            media_type: media.into(),
            ..ProtoData::default()
        };
        (object, proto)
    }

    fn analyze(bytes: &[u8], name: &str, media: &str) -> crate::AnalysisResult {
        let (object, proto) = image(name, media);
        let mut reader = BytesContentReader::from_slice(bytes);
        ExifProvider
            .analyze(&object, &proto, &mut reader)
            .expect("analyze")
    }

    fn push_u16(buf: &mut Vec<u8>, value: u16) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn push_entry(buf: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
        push_u16(buf, tag);
        push_u16(buf, kind);
        push_u32(buf, count);
        push_u32(buf, value);
    }

    fn push_rational(buf: &mut Vec<u8>, num: u32, denom: u32) {
        push_u32(buf, num);
        push_u32(buf, denom);
    }

    /// Little-endian TIFF: primary IFD, Exif IFD, GPS IFD, and a MakerNote blob.
    fn sample_tiff() -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        push_u32(&mut tiff, 8);

        push_u16(&mut tiff, 5);
        push_entry(&mut tiff, 0x010F, 2, 7, 74);
        push_entry(&mut tiff, 0x0110, 2, 4, u32::from_le_bytes(*b"Cam\0"));
        push_entry(&mut tiff, 0x0131, 2, 4, u32::from_le_bytes(*b"App\0"));
        push_entry(&mut tiff, 0x8769, 4, 1, 81);
        push_entry(&mut tiff, 0x8825, 4, 1, 131);
        push_u32(&mut tiff, 0);
        tiff.extend_from_slice(b"Gnosis\0");

        push_u16(&mut tiff, 2);
        push_entry(&mut tiff, 0x9003, 2, 20, 111);
        push_entry(&mut tiff, 0x927C, 7, 4, u32::from_le_bytes([1, 2, 3, 4]));
        push_u32(&mut tiff, 0);
        tiff.extend_from_slice(b"2020:01:02 03:04:05\0");

        push_u16(&mut tiff, 4);
        push_entry(&mut tiff, 0x0001, 2, 2, u32::from_le_bytes(*b"N\0\0\0"));
        push_entry(&mut tiff, 0x0002, 5, 3, 185);
        push_entry(&mut tiff, 0x0003, 2, 2, u32::from_le_bytes(*b"E\0\0\0"));
        push_entry(&mut tiff, 0x0004, 5, 3, 209);
        push_u32(&mut tiff, 0);
        push_rational(&mut tiff, 10, 1);
        push_rational(&mut tiff, 30, 1);
        push_rational(&mut tiff, 0, 1);
        push_rational(&mut tiff, 20, 1);
        push_rational(&mut tiff, 15, 1);
        push_rational(&mut tiff, 0, 1);
        assert_eq!(tiff.len(), 233);
        tiff
    }

    fn sample_jpeg() -> Vec<u8> {
        let tiff = sample_tiff();
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        let length = (2 + 6 + tiff.len()) as u16;
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    fn no_exif_jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }

    fn truncated_jpeg() -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x0C];
        jpeg.extend_from_slice(b"Exif\0\0II*\0");
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn jpeg_with_exif_ifd_and_gps_is_understood() {
        let jpeg = include_bytes!("../../../fixtures/images/with-exif.jpg");
        assert_eq!(jpeg.as_slice(), sample_jpeg().as_slice());
        let result = analyze(jpeg, "photos/camera.jpg", "image/jpeg");
        assert_eq!(result.status, UnderstandingStatus::Understood);
        let image = &result.record.entities[0];
        assert_eq!(image.kind, "image");
        assert_eq!(
            image.attributes.get("make").map(String::as_str),
            Some("Gnosis")
        );
        assert_eq!(
            image.attributes.get("model").map(String::as_str),
            Some("Cam")
        );
        assert_eq!(
            image
                .attributes
                .get("datetime_original")
                .map(String::as_str),
            Some("2020-01-02 03:04:05")
        );
        assert_eq!(
            image.attributes.get("gps_latitude").map(String::as_str),
            Some("10.500000")
        );
        assert_eq!(
            image.attributes.get("gps_longitude").map(String::as_str),
            Some("20.250000")
        );
        assert!(image
            .attributes
            .keys()
            .all(|key| !key.to_ascii_lowercase().contains("makernote")));
        assert_eq!(
            image.attributes.get("exif.Software").map(String::as_str),
            Some("App")
        );
    }

    #[test]
    fn tiff_uses_the_same_ifds() {
        let result = analyze(&sample_tiff(), "photos/camera.tif", "image/tiff");
        assert_eq!(result.status, UnderstandingStatus::Understood);
        assert_eq!(
            result.record.entities[0]
                .attributes
                .get("make")
                .map(String::as_str),
            Some("Gnosis")
        );
    }

    #[test]
    fn jpeg_without_app1_is_partial() {
        let jpeg = include_bytes!("../../../fixtures/images/no-exif.jpg");
        assert_eq!(jpeg.as_slice(), no_exif_jpeg().as_slice());
        let result = analyze(jpeg, "photos/plain.jpg", "image/jpeg");
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(result.record.entities.len(), 1);
        assert_eq!(result.record.entities[0].kind, "image");
        assert!(result
            .record
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("no EXIF")));
    }

    #[test]
    fn truncated_exif_is_partial_and_does_not_panic() {
        let result = analyze(&truncated_jpeg(), "photos/broken.jpg", "image/jpeg");
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(result.record.entities[0].kind, "image");
        assert!(!result.record.diagnostics.is_empty());
    }

    #[test]
    fn probe_claims_jpeg_and_tiff_only() {
        let proto = ProtoData::default();
        let (jpeg, _) = image("a.jpg", "application/octet-stream");
        let (jpeg_media, _) = image("blob.bin", "image/jpeg");
        let (tiff, _) = image("a.tiff", "application/octet-stream");
        let (tiff_media, _) = image("blob.bin", "image/tiff");
        let (png, _) = image("a.png", "image/png");
        assert_eq!(ExifProvider.probe(&jpeg, &proto), Support::Full);
        assert_eq!(ExifProvider.probe(&jpeg_media, &proto), Support::Full);
        assert_eq!(ExifProvider.probe(&tiff, &proto), Support::Full);
        assert_eq!(ExifProvider.probe(&tiff_media, &proto), Support::Full);
        assert_eq!(ExifProvider.probe(&png, &proto), Support::None);
    }

    #[test]
    fn registry_prefers_exif_over_generic() {
        let registry = default_registry();
        let proto = ProtoData::default();
        let (jpeg, _) = image("photos/camera.jpg", "image/jpeg");
        let (png, _) = image("photos/screen.png", "image/png");
        let (provider, support) = registry.select(&jpeg, &proto).expect("jpeg provider");
        assert_eq!(provider.id().as_str(), "exif");
        assert_eq!(support, Support::Full);
        let (provider, _) = registry.select(&png, &proto).expect("png provider");
        assert_eq!(provider.id().as_str(), "generic-metadata");
    }

    #[cfg(feature = "llm")]
    #[test]
    fn exif_full_wins_over_llm_modes() {
        use crate::config::{LlmConfig, LlmMode};
        use crate::providers::build_registry;
        let proto = ProtoData::default();
        let (jpeg, _) = image("photos/camera.jpg", "image/jpeg");
        for mode in [LlmMode::Fallback, LlmMode::Always] {
            let cfg = LlmConfig::new("tinyllama", mode);
            let registry = build_registry(Some(&cfg), None);
            let (provider, support) = registry.select(&jpeg, &proto).expect("jpeg provider");
            assert_eq!(provider.id().as_str(), "exif");
            assert_eq!(support, Support::Full);
        }
    }
}
