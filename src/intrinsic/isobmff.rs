//! MOV/MP4/M4V container metadata. No frame decode.

use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use nom_exif::{EntryValue, MediaParser, MediaSource, TrackInfo, TrackInfoTag};

pub struct IsobmffExtractor;

impl IntrinsicExtractor for IsobmffExtractor {
    fn id(&self) -> &str {
        "isobmff"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "isobmff" || object.extension.as_deref().is_some_and(is_isobmff_ext)
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        track_facts(ctx.bytes, self.id(), self.version(), "isobmff", true)
    }
}

fn is_isobmff_ext(extension: &str) -> bool {
    matches!(extension, "mov" | "mp4" | "m4v")
}

pub(super) fn track_facts(
    bytes: &[u8],
    extractor: &str,
    version: &str,
    prefix: &str,
    include_creation: bool,
) -> crate::Result<Vec<MetadataFact>> {
    let source = MediaSource::from_memory(bytes.to_vec()).map_err(|err| {
        crate::GnosisError::provider(extractor, format!("container sniff failed: {err}"))
    })?;
    let info = MediaParser::new().parse_track(source).map_err(|err| {
        crate::GnosisError::provider(extractor, format!("container metadata not found: {err}"))
    })?;
    let mut facts = Vec::new();
    if let Some(EntryValue::U64(ms)) = info.get(TrackInfoTag::DurationMs) {
        facts.push(MetadataFact::observed(
            format!("{prefix}:duration_ms"),
            extractor,
            version,
            "mvhd:duration",
            FactValue::Integer(i64::try_from(*ms).unwrap_or(i64::MAX)),
        ));
    }
    push_dimension(
        &mut facts,
        &info,
        TrackInfoTag::Width,
        prefix,
        "width",
        extractor,
        version,
    );
    push_dimension(
        &mut facts,
        &info,
        TrackInfoTag::Height,
        prefix,
        "height",
        extractor,
        version,
    );
    if include_creation {
        if let Some(EntryValue::DateTime(time)) = info.get(TrackInfoTag::CreateDate) {
            facts.push(MetadataFact::observed(
                format!("{prefix}:creation_time"),
                extractor,
                version,
                "mvhd:creation_time",
                FactValue::Timestamp(time.to_rfc3339()),
            ));
        }
    }
    if let Some(gps) = info.gps_info() {
        if let Some(latitude) = gps.latitude_decimal() {
            facts.push(sensitive(
                format!("{prefix}:gps_latitude"),
                FactValue::Decimal(format!("{latitude:.6}")),
                extractor,
                version,
            ));
        }
        if let Some(longitude) = gps.longitude_decimal() {
            facts.push(sensitive(
                format!("{prefix}:gps_longitude"),
                FactValue::Decimal(format!("{longitude:.6}")),
                extractor,
                version,
            ));
        }
        if let Some(altitude) = gps.altitude_meters() {
            facts.push(sensitive(
                format!("{prefix}:gps_altitude"),
                FactValue::Decimal(format!("{altitude:.3}")),
                extractor,
                version,
            ));
        }
    }
    if facts.is_empty() {
        return Err(crate::GnosisError::provider(
            extractor,
            "container metadata not found",
        ));
    }
    Ok(facts)
}

fn push_dimension(
    facts: &mut Vec<MetadataFact>,
    info: &TrackInfo,
    tag: TrackInfoTag,
    prefix: &str,
    name: &str,
    extractor: &str,
    version: &str,
) {
    let Some(EntryValue::U32(pixels)) = info.get(tag) else {
        return;
    };
    facts.push(MetadataFact::observed(
        format!("{prefix}:{name}"),
        extractor,
        version,
        "tkhd",
        FactValue::Integer(i64::from(*pixels)),
    ));
}

fn sensitive(key: String, value: FactValue, extractor: &str, version: &str) -> MetadataFact {
    let mut fact = MetadataFact::observed(key, extractor, version, "udta:gps", value);
    fact.sensitive = true;
    fact
}
