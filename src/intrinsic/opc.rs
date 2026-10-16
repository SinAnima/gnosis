//! OPC core and app properties. The structural graph stays on `OfficeProvider`.

use super::{ExtractContext, FactValue, FileSignature, IntrinsicExtractor, MetadataFact};
use crate::connectors::ObjectDescriptor;
use crate::providers::office::opc::{core_props, open_opc};

pub struct OpcExtractor;

impl IntrinsicExtractor for OpcExtractor {
    fn id(&self) -> &str {
        "opc"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, _signature: &FileSignature) -> bool {
        matches!(
            object
                .extension
                .as_deref()
                .map(|ext| ext.to_ascii_lowercase())
                .as_deref(),
            Some("docx" | "xlsx" | "pptx" | "docm" | "xlsm" | "pptm")
        )
    }

    fn extract(&self, ctx: &ExtractContext<'_>) -> crate::Result<Vec<MetadataFact>> {
        let package = open_opc(ctx.bytes)
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        let props = core_props(&package)
            .map_err(|err| crate::GnosisError::provider(self.id(), err.to_string()))?;
        let mut facts = Vec::new();
        push(
            self,
            &mut facts,
            "opc:title",
            "docProps/core.xml",
            props.title.map(FactValue::String),
        );
        push(
            self,
            &mut facts,
            "opc:creator",
            "docProps/core.xml",
            props.creator.map(FactValue::String),
        );
        push(
            self,
            &mut facts,
            "opc:subject",
            "docProps/core.xml",
            props.subject.map(FactValue::String),
        );
        push(
            self,
            &mut facts,
            "opc:modified",
            "docProps/core.xml",
            props.modified.map(FactValue::Timestamp),
        );
        push(
            self,
            &mut facts,
            "opc:revision",
            "docProps/core.xml",
            props.revision.map(FactValue::String),
        );
        push(
            self,
            &mut facts,
            "opc:application",
            "docProps/app.xml",
            props.application.map(FactValue::String),
        );
        Ok(facts)
    }
}

fn push(
    extractor: &OpcExtractor,
    facts: &mut Vec<MetadataFact>,
    key: &str,
    locator: &str,
    value: Option<FactValue>,
) {
    if let Some(value) = value {
        facts.push(MetadataFact::observed(
            key,
            extractor.id(),
            extractor.version(),
            locator,
            value,
        ));
    }
}
