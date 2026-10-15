use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use std::io::Cursor;

pub struct PngExtractor;

impl IntrinsicExtractor for PngExtractor {
    fn id(&self) -> &str {
        "png"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "png" || object.extension.as_deref() == Some("png")
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let decoder = png::Decoder::new(Cursor::new(ctx.bytes));
        let reader = decoder
            .read_info()
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        let info = reader.info();
        let mut facts = vec![
            integer(self, "png:width", "chunk:IHDR", info.width as i64),
            integer(self, "png:height", "chunk:IHDR", info.height as i64),
            integer(
                self,
                "png:bit_depth",
                "chunk:IHDR",
                info.bit_depth as u8 as i64,
            ),
            MetadataFact::observed(
                "png:color_type",
                self.id(),
                self.version(),
                "chunk:IHDR",
                FactValue::String(color_type_name(info.color_type).into()),
            ),
        ];
        if info.icc_profile.is_some() {
            facts.push(MetadataFact::observed(
                "png:icc_present",
                self.id(),
                self.version(),
                "chunk:iCCP",
                FactValue::Bool(true),
            ));
        }
        for chunk in info.uncompressed_latin1_text.iter().take(32) {
            push_text(self, &mut facts, "tEXt", &chunk.keyword, chunk.text.clone());
        }
        for chunk in info.compressed_latin1_text.iter().take(32) {
            let text = chunk.get_text().map_err(|err| {
                crate::GnosisError::provider(self.id(), format!("hostile zTXt: {err}"))
            })?;
            push_text(self, &mut facts, "zTXt", &chunk.keyword, text);
        }
        for chunk in info.utf8_text.iter().take(32) {
            let text = chunk.get_text().map_err(|err| {
                crate::GnosisError::provider(self.id(), format!("hostile iTXt: {err}"))
            })?;
            push_text(self, &mut facts, "iTXt", &chunk.keyword, text);
        }
        Ok(facts)
    }
}

fn push_text(
    extractor: &PngExtractor,
    facts: &mut Vec<MetadataFact>,
    chunk: &str,
    keyword: &str,
    text: String,
) {
    if keyword.is_empty() {
        return;
    }
    facts.push(MetadataFact::observed(
        format!("png:text:{keyword}"),
        extractor.id(),
        extractor.version(),
        format!("chunk:{chunk}:{keyword}"),
        FactValue::String(text),
    ));
}

fn integer(extractor: &PngExtractor, key: &str, locator: &str, value: i64) -> MetadataFact {
    MetadataFact::observed(
        key,
        extractor.id(),
        extractor.version(),
        locator,
        FactValue::Integer(value),
    )
}

fn color_type_name(color: png::ColorType) -> &'static str {
    match color {
        png::ColorType::Grayscale => "grayscale",
        png::ColorType::Rgb => "rgb",
        png::ColorType::Indexed => "indexed",
        png::ColorType::GrayscaleAlpha => "grayscale_alpha",
        png::ColorType::Rgba => "rgba",
    }
}
