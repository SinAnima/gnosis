//! HEIC/HEIF still metadata. No HEVC decode.

use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;

pub struct HeifExtractor;

impl IntrinsicExtractor for HeifExtractor {
    fn id(&self) -> &str {
        "heif"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "heif" || object.extension.as_deref().is_some_and(is_heif_ext)
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let mut facts = Vec::new();
        if let Some(brand) = major_brand(ctx.bytes) {
            facts.push(MetadataFact::observed(
                "heif:brand",
                self.id(),
                self.version(),
                "ftyp:major_brand",
                FactValue::String(brand),
            ));
        }
        if let Some((width, height)) = find_ispe(ctx.bytes) {
            facts.push(integer(self, "heif:width", width));
            facts.push(integer(self, "heif:height", height));
        }
        if let Some(tiff) = exif_tiff_slice(ctx.bytes) {
            let mut exif = super::exif::facts_from_exif_tiff(self.id(), self.version(), tiff);
            mark_sensitive_gps(&mut exif);
            facts.extend(exif);
        }
        if facts.is_empty() {
            return Err(crate::GnosisError::provider(
                self.id(),
                "heif container metadata not found",
            ));
        }
        Ok(facts)
    }
}

fn is_heif_ext(extension: &str) -> bool {
    matches!(extension, "heic" | "heif")
}

fn integer(extractor: &HeifExtractor, key: &str, pixels: u32) -> MetadataFact {
    MetadataFact::observed(
        key,
        extractor.id(),
        extractor.version(),
        "ispe",
        FactValue::Integer(i64::from(pixels)),
    )
}

fn mark_sensitive_gps(facts: &mut [MetadataFact]) {
    for fact in facts {
        if fact.key.contains("GPS") || fact.key.contains("gps") {
            fact.sensitive = true;
        }
    }
}

/// Major brand when the declared `ftyp` fits in `bytes`.
pub(super) fn major_brand(bytes: &[u8]) -> Option<String> {
    let body = ftyp_body(bytes)?;
    let brand = body.get(..4)?;
    let text = String::from_utf8_lossy(brand).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Brands inside a size-checked `ftyp`. `None` when the declared box does not fit.
pub(super) fn ftyp_brands(bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    let body = ftyp_body(bytes)?;
    if body.len() < 8 {
        return None;
    }
    let mut brands = Vec::new();
    brands.push(body[0..4].try_into().ok()?);
    let mut index = 8;
    while index + 4 <= body.len() && brands.len() < 32 {
        brands.push(body[index..index + 4].try_into().ok()?);
        index += 4;
    }
    Some(brands)
}

fn ftyp_body(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return None;
    }
    let size = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
    if size < 16 || size > bytes.len() {
        return None;
    }
    Some(&bytes[8..size])
}

fn exif_tiff_slice(bytes: &[u8]) -> Option<&[u8]> {
    let marker = b"Exif\0\0";
    let mut index = 0;
    while index + marker.len() + 8 <= bytes.len() {
        let found = bytes[index..]
            .windows(marker.len())
            .position(|window| window == marker)?;
        let start = index + found;
        let after = bytes.get(start + marker.len()..)?;
        if after.starts_with(b"II*\x00") || after.starts_with(b"MM\x00*") {
            return bytes.get(start..);
        }
        index = start + 1;
    }
    None
}

fn find_ispe(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut found = None;
    walk_boxes(bytes, 0, &mut |kind, body| {
        if kind == *b"ispe" && body.len() >= 12 {
            let Ok(width) = body[4..8].try_into() else {
                return true;
            };
            let Ok(height) = body[8..12].try_into() else {
                return true;
            };
            found = Some((u32::from_be_bytes(width), u32::from_be_bytes(height)));
            return false;
        }
        true
    });
    found
}

/// `true` means keep walking.
fn walk_boxes(bytes: &[u8], depth: u8, visit: &mut dyn FnMut([u8; 4], &[u8]) -> bool) -> bool {
    if depth > 6 {
        return true;
    }
    let mut index = 0;
    let mut seen = 0u8;
    while index + 8 <= bytes.len() && seen < 64 {
        let Ok(raw) = bytes[index..index + 4].try_into() else {
            break;
        };
        let size = u32::from_be_bytes(raw) as usize;
        if size < 8 || index + size > bytes.len() {
            break;
        }
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&bytes[index + 4..index + 8]);
        let body = &bytes[index + 8..index + size];
        if !visit(kind, body) {
            return false;
        }
        let inner = match &kind {
            b"meta" if body.len() > 4 => Some(&body[4..]),
            b"iprp" | b"ipco" | b"moov" | b"trak" | b"mdia" => Some(body),
            _ => None,
        };
        if let Some(inner) = inner {
            if !walk_boxes(inner, depth + 1, visit) {
                return false;
            }
        }
        index += size;
        seen += 1;
    }
    true
}
