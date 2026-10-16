//! PDF Info-dictionary facts. The structural graph stays on `PdfProvider`.

use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use crate::providers::pdf::parse::{parse, PdfDoc};

pub struct PdfExtractor;

impl IntrinsicExtractor for PdfExtractor {
    fn id(&self) -> &str {
        "pdf"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, signature: &FileSignature) -> bool {
        signature.sniffed == "pdf"
            || object.media_type.eq_ignore_ascii_case("application/pdf")
            || object
                .extension
                .as_deref()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let doc = parse(ctx.bytes)
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        Ok(facts(self, &doc))
    }
}

fn facts(extractor: &PdfExtractor, doc: &PdfDoc) -> Vec<MetadataFact> {
    let mut found = Vec::new();
    push_string(
        extractor,
        &mut found,
        "pdf:title",
        "Info",
        doc.title.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:author",
        "Info",
        doc.author.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:subject",
        "Info",
        doc.subject.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:creator",
        "Info",
        doc.creator.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:producer",
        "Info",
        doc.producer.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:creation_date",
        "Info",
        doc.creation_date.clone(),
    );
    push_string(
        extractor,
        &mut found,
        "pdf:mod_date",
        "Info",
        doc.mod_date.clone(),
    );
    if let Some(count) = doc.page_count {
        found.push(MetadataFact::observed(
            "pdf:page_count",
            extractor.id(),
            extractor.version(),
            "catalog",
            FactValue::Integer(count),
        ));
    }
    found.push(MetadataFact::observed(
        "pdf:encrypted",
        extractor.id(),
        extractor.version(),
        "trailer",
        FactValue::Bool(doc.encrypted),
    ));
    found.push(MetadataFact::observed(
        "pdf:signature_present",
        extractor.id(),
        extractor.version(),
        "catalog",
        FactValue::Bool(doc.signature_present),
    ));
    if doc.xmp_present {
        found.push(MetadataFact::observed(
            "pdf:xmp_present",
            extractor.id(),
            extractor.version(),
            "catalog",
            FactValue::Bool(true),
        ));
    }
    found
}

fn push_string(
    extractor: &PdfExtractor,
    facts: &mut Vec<MetadataFact>,
    key: &str,
    locator: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        facts.push(MetadataFact::observed(
            key,
            extractor.id(),
            extractor.version(),
            locator,
            FactValue::String(value),
        ));
    }
}
