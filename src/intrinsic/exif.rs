use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use crate::providers::ExifProvider;
use crate::{BytesContentReader, UnderstandingProvider};

pub struct ExifExtractor;

impl IntrinsicExtractor for ExifExtractor {
    fn id(&self) -> &str {
        "exif"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        matches!(signature.sniffed.as_str(), "jpeg" | "tiff")
            || matches!(
                object.extension.as_deref(),
                Some("jpg" | "jpeg" | "tif" | "tiff")
            )
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let provider = ExifProvider;
        let mut reader = BytesContentReader::new(ctx.bytes.to_vec());
        let result = provider.analyze(ctx.object, ctx.proto, &mut reader)?;
        let mut facts = Vec::new();
        for entity in &result.record.entities {
            for (key, value) in &entity.attributes {
                if key == "format" {
                    continue;
                }
                let namespaced = namespace_key(key);
                let typed = if key == "datetime_original" || key == "exif.DateTimeOriginal" {
                    FactValue::Timestamp(value.clone())
                } else if key.starts_with("gps_") {
                    FactValue::Decimal(value.clone())
                } else if key == "orientation" {
                    FactValue::Integer(value.parse().unwrap_or(0))
                } else {
                    FactValue::String(value.clone())
                };
                facts.push(MetadataFact::observed(
                    namespaced,
                    self.id(),
                    self.version(),
                    format!("ifd:{key}"),
                    typed,
                ));
            }
        }
        if facts.is_empty() {
            return Err(crate::GnosisError::provider(
                self.id(),
                "image has no EXIF segment",
            ));
        }
        Ok(facts)
    }
}

/// EXIF facts from a TIFF payload, including a WebP EXIF chunk.
pub(super) fn facts_from_exif_tiff(
    extractor_id: &str,
    version: &str,
    bytes: &[u8],
) -> Vec<MetadataFact> {
    let payload = bytes
        .strip_prefix(b"Exif\x00\x00")
        .unwrap_or(bytes)
        .to_vec();
    let parsed = exif::Reader::new()
        .continue_on_error(true)
        .read_raw(payload);
    let exif = match parsed {
        Ok(exif) => exif,
        Err(error) => match error.distill_partial_result(|_| {}) {
            Ok(exif) => exif,
            Err(_) => return Vec::new(),
        },
    };
    let mut facts = Vec::new();
    for field in exif.fields() {
        if field.ifd_num == exif::In::THUMBNAIL || skip_exif_field(field) {
            continue;
        }
        let text = field
            .display_value()
            .to_string()
            .trim()
            .trim_matches('"')
            .to_string();
        if text.is_empty() {
            continue;
        }
        let key = format!("exif:{}", field.tag);
        let value = if field.tag == exif::Tag::DateTimeOriginal {
            FactValue::Timestamp(text)
        } else if field.tag == exif::Tag::Orientation {
            FactValue::Integer(text.parse().unwrap_or(0))
        } else {
            FactValue::String(text)
        };
        facts.push(MetadataFact::observed(
            key,
            extractor_id,
            version,
            format!("ifd:{}", field.tag),
            value,
        ));
        if facts.len() >= 64 {
            break;
        }
    }
    facts
}

fn skip_exif_field(field: &exif::Field) -> bool {
    matches!(
        field.tag,
        exif::Tag::MakerNote
            | exif::Tag::JPEGInterchangeFormat
            | exif::Tag::JPEGInterchangeFormatLength
    ) || matches!(field.value, exif::Value::Undefined(_, _))
        || matches!(field.value, exif::Value::Byte(ref bytes) if bytes.len() > 16)
}

fn namespace_key(key: &str) -> String {
    match key {
        "make" => "exif:Make".into(),
        "model" => "exif:Model".into(),
        "datetime_original" => "exif:DateTimeOriginal".into(),
        "orientation" => "exif:Orientation".into(),
        "gps_latitude" => "exif:GPSLatitude".into(),
        "gps_longitude" => "exif:GPSLongitude".into(),
        "gps_altitude" => "exif:GPSAltitude".into(),
        other => {
            if let Some(tag) = other.strip_prefix("exif.") {
                format!("exif:{tag}")
            } else {
                format!("exif:{other}")
            }
        }
    }
}
