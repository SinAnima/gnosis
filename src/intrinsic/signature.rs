use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;

pub struct SignatureExtractor;

impl IntrinsicExtractor for SignatureExtractor {
    fn id(&self) -> &str {
        "file-signature"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, _object: &ObjectDescriptor, _signature: &FileSignature) -> bool {
        true
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        Ok(vec![
            MetadataFact::observed(
                "file:declared_media",
                self.id(),
                self.version(),
                "header:declared",
                FactValue::String(ctx.signature.declared_media.clone()),
            ),
            MetadataFact::observed(
                "file:sniffed",
                self.id(),
                self.version(),
                "header:magic",
                FactValue::String(ctx.signature.sniffed.clone()),
            ),
        ])
    }
}
