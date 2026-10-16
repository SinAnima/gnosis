//! Structural reader for classic PDF files.
//!
//! User-facing status stays `partial`. Semantic coverage stays `not_attempted`.
//! No page is rendered, no OCR text is invented, no password is attempted, and
//! no JavaScript or action is run.

pub(crate) mod parse;

use std::collections::BTreeSet;
use std::path::Path;

use crate::providers::util::{attrs, entity, relationship};
use crate::{
    AnalysisResult, AttributeMap, Confidence, ContentReader, Diagnostic, DiagnosticSeverity,
    Entity, KnowledgeRecord, ObjectDescriptor, ProtoData, ProviderId, Relationship, Result,
    Support, UnderstandingProvider, UnderstandingStatus,
};

use parse::{parse, PdfDoc};

const REASON: &str = "pdf structure; semantic understanding not attempted";

pub struct PdfProvider;

impl UnderstandingProvider for PdfProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("pdf")
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        // Probe sees the descriptor, not the sniffed bytes. The intrinsic
        // extractor claims a `%PDF` sniff; this reader claims the extension
        // and the `application/pdf` media type.
        let extension = object
            .extension
            .as_deref()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
        let media = object.media_type.eq_ignore_ascii_case("application/pdf");
        if extension || media {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let mut bytes = Vec::new();
        content.read_to_end(&mut bytes)?;
        let doc = match parse(&bytes) {
            Ok(doc) => doc,
            Err(err) => {
                return Ok(finish(
                    UnderstandingStatus::Failed,
                    &err.to_string(),
                    Vec::new(),
                    Vec::new(),
                    vec![error_note(&err.to_string())],
                ));
            }
        };
        if doc.encrypted {
            return Ok(finish(
                UnderstandingStatus::PartiallyUnderstood,
                "encrypted",
                Vec::new(),
                Vec::new(),
                vec![note("encrypted")],
            ));
        }
        let (entities, relationships) = graph(&doc, object, &self.id());
        Ok(finish(
            UnderstandingStatus::PartiallyUnderstood,
            REASON,
            entities,
            relationships,
            Vec::new(),
        ))
    }
}

fn graph(
    doc: &PdfDoc,
    object: &ObjectDescriptor,
    provider: &ProviderId,
) -> (Vec<Entity>, Vec<Relationship>) {
    let path = &object.relative_path;
    let mut used = BTreeSet::new();
    let name = unique_name(&mut used, doc.title.as_deref().unwrap_or(&stem(path)));
    let mut doc_attrs = attrs(&[("index", "1")]);
    join_attr(&mut doc_attrs, "links", &doc.links);
    join_attr(&mut doc_attrs, "fields", &doc.fields);
    join_attr(&mut doc_attrs, "attachments", &doc.attachments);
    join_attr(&mut doc_attrs, "fonts", &doc.fonts);
    let document = entity(
        "document",
        &name,
        path,
        &object.id,
        provider,
        1,
        doc_attrs,
        Confidence::High,
    );
    let mut entities = vec![document.clone()];
    let mut relationships = Vec::new();
    for (offset, page) in doc.pages.iter().enumerate() {
        let index = (offset as u32) + 1;
        let index_s = index.to_string();
        let page_name = unique_name(&mut used, &index_s);
        let mut page_attrs = attrs(&[
            ("page", index_s.as_str()),
            ("index", index_s.as_str()),
            ("width", page.width.as_str()),
            ("height", page.height.as_str()),
            ("rotate", page.rotate.as_str()),
        ]);
        if let Some(text) = &page.embedded_text {
            page_attrs.insert("embedded_text".into(), text.clone());
        }
        let child = entity(
            "page",
            &page_name,
            path,
            &object.id,
            provider,
            index,
            page_attrs,
            Confidence::High,
        );
        relationships.push(relationship(
            "contains",
            &document.id,
            &child.id,
            provider,
            path,
            index,
            Confidence::High,
        ));
        entities.push(child);
    }
    for (offset, title) in doc.bookmarks.iter().enumerate() {
        let index = (offset as u32) + 1;
        let index_s = index.to_string();
        let bookmark_name = unique_name(&mut used, title);
        let child = entity(
            "bookmark",
            &bookmark_name,
            path,
            &object.id,
            provider,
            index,
            attrs(&[("index", index_s.as_str())]),
            Confidence::High,
        );
        relationships.push(relationship(
            "has_bookmark",
            &document.id,
            &child.id,
            provider,
            path,
            index,
            Confidence::High,
        ));
        entities.push(child);
    }
    (entities, relationships)
}

fn join_attr(map: &mut AttributeMap, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    map.insert(key.into(), values.join("\n"));
}

fn unique_name(used: &mut BTreeSet<String>, name: &str) -> String {
    let name = if name.is_empty() {
        "untitled".to_string()
    } else {
        name.to_string()
    };
    if used.insert(name.clone()) {
        return name;
    }
    let mut suffix = 2u32;
    loop {
        let candidate = format!("{name} #{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into())
}

fn finish(
    status: UnderstandingStatus,
    reason: &str,
    entities: Vec<Entity>,
    relationships: Vec<Relationship>,
    diagnostics: Vec<Diagnostic>,
) -> AnalysisResult {
    AnalysisResult {
        record: KnowledgeRecord {
            entities,
            relationships,
            diagnostics,
            status: Some(status),
            classification_reason: Some(reason.into()),
        },
        status,
        classification_reason: Some(reason.into()),
    }
}

fn note(message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Warning,
        message: message.into(),
    }
}

fn error_note(message: &str) -> Diagnostic {
    Diagnostic {
        severity: DiagnosticSeverity::Error,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse::PdfError;
    use super::*;
    use crate::intrinsic::{extract_intrinsic, FactValue};
    use crate::{BytesContentReader, ObjectId};
    use std::io::Write;
    use std::path::PathBuf;

    fn object(path: &str) -> ObjectDescriptor {
        let extension = Path::new(path)
            .extension()
            .map(|ext| ext.to_string_lossy().into_owned());
        ObjectDescriptor {
            id: ObjectId::new(format!("obj:{path}")),
            path: PathBuf::from(path),
            relative_path: PathBuf::from(path),
            is_dir: false,
            size: 1,
            modified: None,
            media_type: "application/octet-stream".into(),
            extension,
            etag: None,
        }
    }

    fn analyze(path: &str, bytes: &[u8]) -> AnalysisResult {
        let object = object(path);
        let mut reader = BytesContentReader::from_slice(bytes);
        PdfProvider
            .analyze(&object, &ProtoData::default(), &mut reader)
            .unwrap()
    }

    fn pdf_from(objects: &[Vec<u8>], root: u32, trailer_extra: &str) -> Vec<u8> {
        let mut body = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(body.len());
            let header = format!("{} 0 obj\n", index + 1);
            body.extend_from_slice(header.as_bytes());
            body.extend_from_slice(object);
            if !object.ends_with(b"\n") {
                body.push(b'\n');
            }
            body.extend_from_slice(b"endobj\n");
        }
        let xref_at = body.len();
        let size = objects.len() + 1;
        body.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        body.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            body.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let trailer = format!(
            "trailer\n<< /Size {size} /Root {root} 0 R {trailer_extra} >>\nstartxref\n{xref_at}\n%%EOF\n"
        );
        body.extend_from_slice(trailer.as_bytes());
        body
    }

    fn stream_object(header: &str, data: &[u8]) -> Vec<u8> {
        let mut object = format!("{header}\nstream\n").into_bytes();
        object.extend_from_slice(data);
        object.extend_from_slice(b"\nendstream");
        object
    }

    fn note_pdf() -> Vec<u8> {
        let text = b"BT /F1 12 Tf 72 72 Td (Synthetic note) Tj ET\n";
        let length = text.len().to_string();
        pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R /AcroForm << /Fields [8 0 R] >> /Names << /EmbeddedFiles << /Names [ (secret.txt) 9 0 R ] >> >> >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Rotate 90 /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> /Annots [10 0 R] >>".to_vec(),
                stream_object("<< /Length 5 0 R >>", text),
                length.into_bytes(),
                b"<< /Type /Outlines /First 11 0 R /Count 1 >>".to_vec(),
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
                b"<< /T (Office) /FT /Tx >>".to_vec(),
                b"<< /Type /Filespec /F (secret.txt) /EF << /F 12 0 R >> >>".to_vec(),
                b"<< /Type /Annot /Subtype /Link /A << /S /URI /URI (https://example.com/note) >> >>".to_vec(),
                b"<< /Title (Overview) /Parent 6 0 R >>".to_vec(),
                stream_object("<< /Length 5 >>", b"HELLO"),
                b"<< /Title (Synthetic note) /Author (Gnosis) /Subject (Fixture) /Creator (gnosis) /Producer (gnosis) /CreationDate (D:20260102030405Z) /ModDate (D:20260102030405Z) >>".to_vec(),
            ],
            1,
            "/Info 13 0 R",
        )
    }

    fn page_of(result: &AnalysisResult) -> &Entity {
        result
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "page")
            .unwrap()
    }

    fn document_of(result: &AnalysisResult) -> &Entity {
        result
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "document")
            .unwrap()
    }

    #[test]
    fn note_graph_and_facts() {
        let bytes = note_pdf();
        let result = analyze("office/note.pdf", &bytes);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(
            result.classification_reason.as_deref(),
            Some("pdf structure; semantic understanding not attempted")
        );
        let document = document_of(&result);
        assert_eq!(document.name, "Synthetic note");
        assert_eq!(
            document.attributes.get("links").map(String::as_str),
            Some("https://example.com/note")
        );
        assert_eq!(
            document.attributes.get("fields").map(String::as_str),
            Some("Office")
        );
        assert_eq!(
            document.attributes.get("attachments").map(String::as_str),
            Some("secret.txt")
        );
        assert_eq!(
            document.attributes.get("fonts").map(String::as_str),
            Some("Helvetica")
        );
        assert!(document
            .attributes
            .values()
            .all(|value| !value.contains("HELLO")));
        let page = page_of(&result);
        assert_eq!(page.name, "1");
        assert_eq!(page.attributes["page"], "1");
        assert_eq!(page.attributes["index"], "1");
        assert_eq!(page.attributes["width"], "300");
        assert_eq!(page.attributes["height"], "144");
        assert_eq!(page.attributes["rotate"], "90");
        assert_eq!(page.attributes["embedded_text"], "Synthetic note");
        assert!(result
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "contains"));
        assert!(result
            .record
            .entities
            .iter()
            .any(|entity| entity.kind == "bookmark" && entity.name == "Overview"));
        assert!(result
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "has_bookmark"));

        let record = extract_intrinsic(&object("office/note.pdf"), &ProtoData::default(), &bytes);
        let fact = |key: &str| {
            record
                .intrinsic
                .facts
                .iter()
                .find(|fact| fact.key == key)
                .map(|fact| fact.value.clone())
        };
        assert_eq!(
            fact("pdf:title"),
            Some(FactValue::String("Synthetic note".into()))
        );
        assert_eq!(fact("pdf:author"), Some(FactValue::String("Gnosis".into())));
        assert_eq!(
            fact("pdf:subject"),
            Some(FactValue::String("Fixture".into()))
        );
        assert_eq!(
            fact("pdf:creator"),
            Some(FactValue::String("gnosis".into()))
        );
        assert_eq!(
            fact("pdf:producer"),
            Some(FactValue::String("gnosis".into()))
        );
        assert_eq!(
            fact("pdf:creation_date"),
            Some(FactValue::String("D:20260102030405Z".into()))
        );
        assert_eq!(
            fact("pdf:mod_date"),
            Some(FactValue::String("D:20260102030405Z".into()))
        );
        assert_eq!(fact("pdf:page_count"), Some(FactValue::Integer(1)));
        assert_eq!(fact("pdf:encrypted"), Some(FactValue::Bool(false)));
        assert_eq!(fact("pdf:signature_present"), Some(FactValue::Bool(false)));
        assert!(fact("pdf:xmp_present").is_none());
        assert!(record
            .intrinsic
            .creation_time()
            .iter()
            .any(|fact| fact.key == "pdf:creation_date"));
        assert!(crate::intrinsic::extractor_stamp().contains("pdf:0.1.0"));
        assert!(record
            .intrinsic
            .facts
            .iter()
            .all(|fact| !matches!(&fact.value, FactValue::String(text) if text.contains("HELLO"))));
    }

    #[test]
    fn image_only_page_stays_partial() {
        let text = b"q Q\n";
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>".to_vec(),
                stream_object(&format!("<< /Length {} >>", text.len()), text),
            ],
            1,
            "",
        );
        let result = analyze("office/scan.pdf", &bytes);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_ne!(result.status, UnderstandingStatus::Understood);
        assert_eq!(document_of(&result).name, "scan");
        let page = page_of(&result);
        assert!(!page.attributes.contains_key("embedded_text"));
        assert!(!page.attributes.contains_key("ocr_text"));
    }

    #[test]
    fn rejects_empty_truncated_and_non_pdf() {
        for (bytes, message) in [
            (Vec::new(), "pdf unreadable"),
            (b"hello".to_vec(), "pdf unreadable"),
            (b"%PDF-1.4\n".to_vec(), "pdf truncated"),
        ] {
            let result = analyze("office/note.pdf", &bytes);
            assert_eq!(result.status, UnderstandingStatus::Failed);
            assert_eq!(result.record.diagnostics[0].message, message);
            assert!(result.record.entities.is_empty());
        }
    }

    #[test]
    fn huge_length_is_rejected_before_allocation() {
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_vec(),
                b"<< /Length 9000000000000 >>\nstream\nBT\nendstream".to_vec(),
            ],
            1,
            "",
        );
        assert!(matches!(parse(&bytes), Err(PdfError::Limit)));
        let result = analyze("office/note.pdf", &bytes);
        assert_eq!(result.status, UnderstandingStatus::Failed);
        assert_eq!(
            result.record.diagnostics[0].message,
            "pdf limit uncompressed stream"
        );
    }

    #[test]
    fn encrypted_trailer_is_a_diagnostic() {
        let bytes = pdf_from(
            &[b"<< /Filter /Standard /V 1 /Length 40 >>".to_vec()],
            1,
            "/Encrypt 1 0 R",
        );
        let result = analyze("office/secret.pdf", &bytes);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(result.record.diagnostics[0].message, "encrypted");
        assert!(result.record.entities.is_empty());
        let record = extract_intrinsic(&object("office/secret.pdf"), &ProtoData::default(), &bytes);
        let encrypted = record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "pdf:encrypted")
            .unwrap();
        assert_eq!(encrypted.value, FactValue::Bool(true));
        let signature = record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "pdf:signature_present")
            .unwrap();
        assert_eq!(signature.value, FactValue::Bool(false));
        assert!(record
            .intrinsic
            .facts
            .iter()
            .all(|fact| fact.key.starts_with("pdf:")
                || fact.key.starts_with("file:")
                || fact.key.starts_with("fs:")));
        assert!(record
            .intrinsic
            .facts
            .iter()
            .all(|fact| fact.key != "pdf:title"));
    }

    #[test]
    fn signature_flag_is_not_validated() {
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>".to_vec(),
                b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec(),
                b"<< /T (Approval) /FT /Sig >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>".to_vec(),
            ],
            1,
            "",
        );
        assert!(!bytes.windows(9).any(|window| window == b"ByteRange"));
        let result = analyze("office/signed.pdf", &bytes);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert!(result.record.diagnostics.is_empty());
        let record = extract_intrinsic(&object("office/signed.pdf"), &ProtoData::default(), &bytes);
        let flag = record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "pdf:signature_present")
            .unwrap();
        assert_eq!(flag.value, FactValue::Bool(true));
    }

    #[test]
    fn unicode_title_and_xmp_presence() {
        let packet =
            b"<?xpacket begin=\"?\"?><dc:title>FromXmpOnly</dc:title><?xpacket end=\"w\"?>";
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>".to_vec(),
                b"<< /Title <FEFF00DC00620065007200730069006300680074> >>".to_vec(),
                stream_object(
                    &format!(
                        "<< /Type /Metadata /Subtype /XML /Length {} >>",
                        packet.len()
                    ),
                    packet,
                ),
            ],
            1,
            "/Info 4 0 R",
        );
        let result = analyze("office/note.pdf", &bytes);
        assert_eq!(document_of(&result).name, "Übersicht");
        let record = extract_intrinsic(&object("office/note.pdf"), &ProtoData::default(), &bytes);
        let title = record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "pdf:title")
            .unwrap();
        assert_eq!(title.value, FactValue::String("Übersicht".into()));
        let xmp = record
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "pdf:xmp_present")
            .unwrap();
        assert_eq!(xmp.value, FactValue::Bool(true));
        assert!(record.intrinsic.facts.iter().all(|fact| {
            !matches!(&fact.value, FactValue::String(text) if text.contains("FromXmpOnly"))
        }));
    }

    #[test]
    fn xref_stream_is_unread() {
        let bytes = b"%PDF-1.4\n1 0 obj\n<< /Type /XRef >>\nendobj\nstartxref\n9\n%%EOF\n";
        assert!(matches!(parse(bytes), Err(PdfError::XrefStream)));
        let result = analyze("office/note.pdf", bytes);
        assert_eq!(result.status, UnderstandingStatus::Failed);
        assert_eq!(result.record.diagnostics[0].message, "xref stream unread");
    }

    #[test]
    fn flate_text_and_text_cap() {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"BT (Hello) Tj ET").unwrap();
        let compressed = encoder.finish().unwrap();
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_vec(),
                stream_object(
                    &format!("<< /Filter /FlateDecode /Length {} >>", compressed.len()),
                    &compressed,
                ),
            ],
            1,
            "",
        );
        let result = analyze("office/note.pdf", &bytes);
        assert_eq!(page_of(&result).attributes["embedded_text"], "Hello");

        let long = "A".repeat(2000);
        let text = format!("BT ({long}) Tj ET");
        let capped = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_vec(),
                stream_object(&format!("<< /Length {} >>", text.len()), text.as_bytes()),
            ],
            1,
            "",
        );
        let result = analyze("office/note.pdf", &capped);
        assert_eq!(
            page_of(&result).attributes["embedded_text"].chars().count(),
            1024
        );
    }

    #[test]
    fn sniff_without_extension_still_emits_facts() {
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Count 0 >>".to_vec(),
                b"<< /Title (Sniffed) >>".to_vec(),
            ],
            1,
            "/Info 3 0 R",
        );
        let record = extract_intrinsic(&object("note.bin"), &ProtoData::default(), &bytes);
        assert!(record.intrinsic.facts.iter().any(
            |fact| fact.key == "pdf:title" && fact.value == FactValue::String("Sniffed".into())
        ));
        assert!(matches!(
            PdfProvider.probe(&object("note.bin"), &ProtoData::default()),
            Support::None
        ));
        let mut sniffed = object("note.bin");
        sniffed.media_type = "application/pdf".into();
        assert!(matches!(
            PdfProvider.probe(&sniffed, &ProtoData::default()),
            Support::Full
        ));
    }

    #[test]
    fn tj_array_concatenates_literal_strings() {
        let text = b"BT [(Hello) 20 ( there)] TJ ET";
        let bytes = pdf_from(
            &[
                b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_vec(),
                stream_object(&format!("<< /Length {} >>", text.len()), text),
            ],
            1,
            "",
        );
        let result = analyze("office/note.pdf", &bytes);
        assert_eq!(page_of(&result).attributes["embedded_text"], "Hello there");
    }
}
