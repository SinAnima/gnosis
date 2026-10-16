//! WebM container metadata. No frame decode.

use super::isobmff::track_facts;
use super::{ExtractContext, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;

pub struct WebmExtractor;

impl IntrinsicExtractor for WebmExtractor {
    fn id(&self) -> &str {
        "webm"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "webm" || object.extension.as_deref() == Some("webm")
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        match track_facts(ctx.bytes, self.id(), self.version(), "webm", false) {
            Ok(facts) => Ok(facts),
            Err(_) => Ok(Vec::new()),
        }
    }
}
