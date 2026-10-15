use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use std::io::Cursor;

pub struct AvifExtractor;

impl IntrinsicExtractor for AvifExtractor {
    fn id(&self) -> &str {
        "avif"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "avif" || object.extension.as_deref() == Some("avif")
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let (width, height, bit_depth, still) = {
            let data = avif_parse::read_avif(&mut Cursor::new(ctx.bytes))
                .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
            let meta = data
                .primary_item_metadata()
                .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
            (
                meta.max_frame_width.get(),
                meta.max_frame_height.get(),
                meta.bit_depth,
                meta.still_picture,
            )
        };
        Ok(vec![
            MetadataFact::observed(
                "avif:width",
                self.id(),
                self.version(),
                "obu:sequence_header",
                FactValue::Integer(width as i64),
            ),
            MetadataFact::observed(
                "avif:height",
                self.id(),
                self.version(),
                "obu:sequence_header",
                FactValue::Integer(height as i64),
            ),
            MetadataFact::observed(
                "avif:bit_depth",
                self.id(),
                self.version(),
                "obu:sequence_header",
                FactValue::Integer(bit_depth as i64),
            ),
            MetadataFact::observed(
                "avif:still_picture",
                self.id(),
                self.version(),
                "obu:sequence_header",
                FactValue::Bool(still),
            ),
        ])
    }
}
