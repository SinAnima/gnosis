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
