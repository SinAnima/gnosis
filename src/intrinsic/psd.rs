use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;

pub struct PsdExtractor;

impl IntrinsicExtractor for PsdExtractor {
    fn id(&self) -> &str {
        "psd"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "psd" || object.extension.as_deref() == Some("psd")
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let psd = psd::Psd::from_bytes(ctx.bytes)
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        let names: Vec<String> = psd
            .layers()
            .iter()
            .take(32)
            .map(|layer| layer.name().to_string())
            .collect();
        Ok(vec![
            MetadataFact::observed(
                "psd:width",
                self.id(),
                self.version(),
                "section:file_header",
                FactValue::Integer(psd.width() as i64),
            ),
            MetadataFact::observed(
                "psd:height",
                self.id(),
                self.version(),
                "section:file_header",
                FactValue::Integer(psd.height() as i64),
            ),
            MetadataFact::observed(
                "psd:depth",
                self.id(),
                self.version(),
                "section:file_header",
                FactValue::Integer(depth_bits(psd.depth()) as i64),
            ),
            MetadataFact::observed(
                "psd:color_mode",
                self.id(),
                self.version(),
                "section:file_header",
                FactValue::String(color_mode_name(psd.color_mode()).into()),
            ),
            MetadataFact::observed(
                "psd:layers",
                self.id(),
                self.version(),
                "section:layer_and_mask",
                FactValue::Array(names),
            ),
        ])
    }
}

fn depth_bits(depth: psd::PsdDepth) -> u8 {
    match depth {
        psd::PsdDepth::One => 1,
        psd::PsdDepth::Eight => 8,
        psd::PsdDepth::Sixteen => 16,
        psd::PsdDepth::ThirtyTwo => 32,
    }
}

fn color_mode_name(mode: psd::ColorMode) -> &'static str {
    match mode {
        psd::ColorMode::Bitmap => "bitmap",
        psd::ColorMode::Grayscale => "grayscale",
        psd::ColorMode::Indexed => "indexed",
        psd::ColorMode::Rgb => "rgb",
        psd::ColorMode::Cmyk => "cmyk",
        psd::ColorMode::Multichannel => "multichannel",
        psd::ColorMode::Duotone => "duotone",
        psd::ColorMode::Lab => "lab",
    }
}
