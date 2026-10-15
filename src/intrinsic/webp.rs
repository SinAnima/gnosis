use super::exif::facts_from_exif_tiff;
use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use std::io::{BufReader, Cursor};

pub struct WebpExtractor;

impl IntrinsicExtractor for WebpExtractor {
    fn id(&self) -> &str {
        "webp"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "webp" || object.extension.as_deref() == Some("webp")
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let mut decoder = image_webp::WebPDecoder::new(BufReader::new(Cursor::new(ctx.bytes)))
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        let (width, height) = decoder.dimensions();
        let mut facts = vec![
            MetadataFact::observed(
                "webp:width",
                self.id(),
                self.version(),
                "chunk:VP8X",
                FactValue::Integer(width as i64),
            ),
            MetadataFact::observed(
                "webp:height",
                self.id(),
                self.version(),
                "chunk:VP8X",
                FactValue::Integer(height as i64),
            ),
        ];
        if let Some(exif) = decoder
            .exif_metadata()
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?
        {
            facts.extend(facts_from_exif_tiff(self.id(), self.version(), &exif));
        }
        if let Some(xmp) = decoder
            .xmp_metadata()
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?
        {
            facts.push(MetadataFact::observed(
                "webp:xmp_present",
                self.id(),
                self.version(),
                "chunk:XMP",
                FactValue::Bool(true),
            ));
            facts.push(MetadataFact::observed(
                "webp:xmp_length",
                self.id(),
                self.version(),
                "chunk:XMP",
                FactValue::Integer(xmp.len() as i64),
            ));
        }
        Ok(facts)
    }
}
