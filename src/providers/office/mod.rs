//! Structural reader for DOCX, XLSX, and PPTX packages.

mod excel;
mod formula;
pub(crate) mod opc;
mod slides;
mod word;

use std::collections::BTreeSet;
use std::path::Path;

use crate::providers::util::{attrs, entity, ext_is, relationship};
use crate::{
    AnalysisResult, Confidence, ContentReader, Diagnostic, DiagnosticSeverity, Entity, EntityId,
    KnowledgeRecord, ObjectDescriptor, ProtoData, ProviderId, Relationship, Result, Support,
    UnderstandingProvider, UnderstandingStatus,
};

use excel::read_workbook;
use opc::{open_opc, OpcError, OpcPackage};
use slides::read_deck;
use word::read_word;

const REASON: &str = "office structure; semantic understanding not attempted";

type OfficeGraph = std::result::Result<(Vec<Entity>, Vec<Relationship>, Vec<Diagnostic>), OpcError>;

pub struct OfficeProvider;

impl UnderstandingProvider for OfficeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("office")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["docx", "xlsx", "pptx"]) {
            Support::Full
        } else if ext_is(
            &object.extension,
            &["docm", "xlsm", "pptm", "doc", "xls", "ppt"],
        ) {
            Support::Partial
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
        let ext = object
            .extension
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(ext.as_str(), "doc" | "xls" | "ppt") {
            return Ok(finish(
                UnderstandingStatus::PartiallyUnderstood,
                "unsupported_legacy_format",
                Vec::new(),
                Vec::new(),
                vec![note("unsupported_legacy_format")],
            ));
        }
        let mut bytes = Vec::new();
        content.read_to_end(&mut bytes)?;
        let package = match open_opc(&bytes) {
            Ok(package) => package,
            Err(OpcError::Encrypted) => {
                return Ok(finish(
                    UnderstandingStatus::PartiallyUnderstood,
                    "encrypted",
                    Vec::new(),
                    Vec::new(),
                    vec![note("encrypted")],
                ));
            }
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
        let mut diagnostics = Vec::new();
        if matches!(ext.as_str(), "docm" | "xlsm" | "pptm") || package.skipped_vba {
            diagnostics.push(note("macro_not_executed"));
        }
        let provider = self.id();
        let path = &object.relative_path;
        let fallback = stem(path);
        let built = match ext.as_str() {
            "docx" | "docm" => graph_word(&package, object, &provider, path, &fallback),
            "xlsx" | "xlsm" => graph_excel(&package, object, &provider, path, &fallback),
            "pptx" | "pptm" => graph_slides(&package, object, &provider, path, &fallback),
            _ => Err(OpcError::NotZip),
        };
        match built {
            Ok((entities, relationships, mut extra)) => {
                diagnostics.append(&mut extra);
                Ok(finish(
                    UnderstandingStatus::PartiallyUnderstood,
                    REASON,
                    entities,
                    relationships,
                    diagnostics,
                ))
            }
            Err(err) => Ok(finish(
                UnderstandingStatus::Failed,
                &err.to_string(),
                Vec::new(),
                Vec::new(),
                vec![error_note(&err.to_string())],
            )),
        }
    }
}

fn graph_word(
    package: &OpcPackage,
    object: &ObjectDescriptor,
    provider: &ProviderId,
    path: &Path,
    fallback: &str,
) -> OfficeGraph {
    let doc = read_word(package)?;
    let props = opc::core_props(package)?;
    let mut used = BTreeSet::new();
    let name = unique_name(&mut used, props.title.as_deref().unwrap_or(fallback));
    let mut doc_attrs = attrs(&[("part", doc.part.as_str()), ("index", "1")]);
    if !doc.hyperlinks.is_empty() {
        doc_attrs.insert("hyperlinks".into(), doc.hyperlinks.join("\n"));
    }
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
    for (offset, heading) in doc.headings.iter().enumerate() {
        let index = (offset as u32) + 1;
        let heading_name = unique_name(&mut used, &heading.text);
        let level = heading.level.to_string();
        let index_s = index.to_string();
        let child = entity(
            "heading",
            &heading_name,
            path,
            &object.id,
            provider,
            index,
            attrs(&[
                ("part", doc.part.as_str()),
                ("index", index_s.as_str()),
                ("level", level.as_str()),
            ]),
            Confidence::High,
        );
        relationships.push(relationship(
            "has_heading",
            &document.id,
            &child.id,
            provider,
            path,
            index,
            Confidence::High,
        ));
        entities.push(child);
    }
    Ok((entities, relationships, Vec::new()))
}

fn graph_excel(
    package: &OpcPackage,
    object: &ObjectDescriptor,
    provider: &ProviderId,
    path: &Path,
    fallback: &str,
) -> OfficeGraph {
    let book = read_workbook(package)?;
    let props = opc::core_props(package)?;
    let mut used = BTreeSet::new();
    let mut ranges: Vec<(String, EntityId)> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut external = false;
    let name = unique_name(&mut used, props.title.as_deref().unwrap_or(fallback));
    let workbook = entity(
        "workbook",
        &name,
        path,
        &object.id,
        provider,
        1,
        attrs(&[("part", book.part.as_str()), ("index", "1")]),
        Confidence::High,
    );
    let mut entities = vec![workbook.clone()];
    let mut relationships = Vec::new();
    let mut formula_index = 0u32;
    for sheet in &book.sheets {
        let sheet_name = unique_name(&mut used, &sheet.name);
        let index_s = sheet.index.to_string();
        let worksheet = entity(
            "worksheet",
            &sheet_name,
            path,
            &object.id,
            provider,
            sheet.index,
            attrs(&[
                ("part", sheet.part.as_str()),
                ("index", index_s.as_str()),
                ("visibility", sheet.visibility.as_str()),
            ]),
            Confidence::High,
        );
        relationships.push(relationship(
            "contains",
            &workbook.id,
            &worksheet.id,
            provider,
            path,
            sheet.index,
            Confidence::High,
        ));
        for formula in &sheet.formulas {
            formula_index += 1;
            if formula.external {
                external = true;
            }
            let formula_name = unique_name(&mut used, &format!("{}!{}", sheet.name, formula.cell));
            let index_s = formula_index.to_string();
            let mut formula_attrs = attrs(&[
                ("part", sheet.part.as_str()),
                ("index", index_s.as_str()),
                ("source", formula.source.as_str()),
            ]);
            if formula.external {
                formula_attrs.insert("external".into(), "true".into());
            }
            let formula_entity = entity(
                "formula",
                &formula_name,
                path,
                &object.id,
                provider,
                formula_index,
                formula_attrs,
                Confidence::High,
            );
            relationships.push(relationship(
                "contains",
                &worksheet.id,
                &formula_entity.id,
                provider,
                path,
                formula_index,
                Confidence::High,
            ));
            for reference in &formula.refs {
                let range_id = if let Some((_, id)) = ranges
                    .iter()
                    .find(|(display, _)| display == &reference.display)
                {
                    id.clone()
                } else {
                    let range_index = (ranges.len() as u32) + 1;
                    let range_name = unique_name(&mut used, &reference.display);
                    let index_s = range_index.to_string();
                    let range = entity(
                        "range",
                        &range_name,
                        path,
                        &object.id,
                        provider,
                        range_index,
                        attrs(&[("part", sheet.part.as_str()), ("index", index_s.as_str())]),
                        Confidence::High,
                    );
                    let id = range.id.clone();
                    ranges.push((reference.display.clone(), id.clone()));
                    entities.push(range);
                    id
                };
                relationships.push(relationship(
                    "references",
                    &formula_entity.id,
                    &range_id,
                    provider,
                    path,
                    formula_index,
                    Confidence::High,
                ));
            }
            entities.push(formula_entity);
        }
        entities.push(worksheet);
    }
    if external {
        diagnostics.push(note("unresolved external reference"));
    }
    Ok((entities, relationships, diagnostics))
}

fn graph_slides(
    package: &OpcPackage,
    object: &ObjectDescriptor,
    provider: &ProviderId,
    path: &Path,
    fallback: &str,
) -> OfficeGraph {
    let deck = read_deck(package)?;
    let props = opc::core_props(package)?;
    let mut used = BTreeSet::new();
    let name = unique_name(&mut used, props.title.as_deref().unwrap_or(fallback));
    let presentation = entity(
        "presentation",
        &name,
        path,
        &object.id,
        provider,
        1,
        attrs(&[("part", deck.part.as_str()), ("index", "1")]),
        Confidence::High,
    );
    let mut entities = vec![presentation.clone()];
    let mut relationships = Vec::new();
    for slide in &deck.slides {
        let title = if slide.title.is_empty() {
            format!("slide {}", slide.id)
        } else {
            slide.title.clone()
        };
        let slide_name = unique_name(&mut used, &title);
        let index_s = slide.index.to_string();
        let mut slide_attrs = attrs(&[
            ("part", slide.part.as_str()),
            ("index", index_s.as_str()),
            ("id", slide.id.as_str()),
        ]);
        if let Some(notes) = &slide.notes {
            slide_attrs.insert("notes".into(), notes.clone());
        }
        let child = entity(
            "slide",
            &slide_name,
            path,
            &object.id,
            provider,
            slide.index,
            slide_attrs,
            Confidence::High,
        );
        relationships.push(relationship(
            "contains",
            &presentation.id,
            &child.id,
            provider,
            path,
            slide.index,
            Confidence::High,
        ));
        entities.push(child);
    }
    Ok((entities, relationships, Vec::new()))
}

pub(crate) fn part_named<'a>(
    package: &'a OpcPackage,
    fallback: &str,
) -> Option<(&'a str, &'a [u8])> {
    let file = fallback.rsplit('/').next().unwrap_or(fallback);
    if let Some(rels) = package.rels.get("_rels/.rels") {
        for rel in rels {
            if rel.target.ends_with(file) {
                if let Some((key, bytes)) = package.parts.get_key_value(&rel.target) {
                    return Some((key.as_str(), bytes.as_slice()));
                }
            }
        }
    }
    package
        .parts
        .get_key_value(fallback)
        .map(|(key, bytes)| (key.as_str(), bytes.as_slice()))
}

pub(crate) fn rels_path(part: &str) -> String {
    match part.rfind('/') {
        Some(index) => format!("{}/_rels/{}.rels", &part[..index], &part[index + 1..]),
        None => format!("_rels/{part}.rels"),
    }
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
    use super::*;
    use crate::intrinsic::{extract_intrinsic, FactValue};
    use crate::{BytesContentReader, ObjectId, ProtoData};
    use std::io::{Cursor, Write};
    use std::path::PathBuf;

    fn stored(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, body) in parts {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

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
        OfficeProvider
            .analyze(&object, &ProtoData::default(), &mut reader)
            .unwrap()
    }

    fn names(result: &AnalysisResult, kind: &str) -> Vec<String> {
        result
            .record
            .entities
            .iter()
            .filter(|entity| entity.kind == kind)
            .map(|entity| entity.name.clone())
            .collect()
    }

    fn core() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/">
  <dc:title>Synthetic brief</dc:title>
  <dc:creator>Gnosis</dc:creator>
  <dc:subject>Catalog</dc:subject>
  <cp:revision>3</cp:revision>
  <dcterms:modified>2026-01-02T03:04:05Z</dcterms:modified>
</cp:coreProperties>"#
    }

    fn app() -> &'static str {
        r#"<?xml version="1.0" encoding="UTF-8"?><Properties><Application>Gnosis</Application></Properties>"#
    }

    #[test]
    fn reads_heading_formula_and_slide() {
        let docx = stored(&[
            ("[Content_Types].xml", "<Types></Types>"),
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
            ),
            ("docProps/core.xml", core()),
            ("docProps/app.xml", app()),
            (
                "word/_rels/document.xml.rels",
                r#"<Relationships><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/catalog" TargetMode="External"/></Relationships>"#,
            ),
            (
                "word/document.xml",
                r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:rPr><w:vanish/></w:rPr><w:t>SECRET</w:t></w:r>
      <w:del><w:r><w:delText>GONE</w:delText></w:r></w:del>
      <w:r><w:t>Übersicht</w:t></w:r>
      <w:hyperlink r:id="rId5"><w:r><w:t></w:t></w:r></w:hyperlink>
    </w:p>
  </w:body>
</w:document>"#,
            ),
        ]);
        let word = analyze("office/brief.docx", &docx);
        assert_eq!(word.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(names(&word, "document"), vec!["Synthetic brief"]);
        assert_eq!(names(&word, "heading"), vec!["Übersicht"]);
        assert!(word
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "has_heading"));
        let document = word
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "document")
            .unwrap();
        assert_eq!(
            document.attributes.get("hyperlinks").map(String::as_str),
            Some("https://example.com/catalog")
        );
        assert!(!word
            .record
            .entities
            .iter()
            .any(|entity| entity.name.contains("SECRET") || entity.name.contains("GONE")));

        let xlsx = stored(&[
            ("[Content_Types].xml", "<Types></Types>"),
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="价格" sheetId="1" r:id="rId1"/>
    <sheet name="Secret" sheetId="2" state="hidden" r:id="rId2"/>
  </sheets>
</workbook>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData><row><c r="B2"><f>价格!A1</f><v>9</v></c></row></sheetData>
</worksheet>"#,
            ),
        ]);
        let excel = analyze("office/book.xlsx", &xlsx);
        assert_eq!(
            names(&excel, "worksheet"),
            vec!["价格".to_string(), "Secret".to_string()]
        );
        let hidden = excel
            .record
            .entities
            .iter()
            .find(|entity| entity.name == "Secret")
            .unwrap();
        assert_eq!(
            hidden.attributes.get("visibility").map(String::as_str),
            Some("hidden")
        );
        assert_eq!(names(&excel, "formula"), vec!["价格!B2"]);
        assert_eq!(names(&excel, "range"), vec!["价格!A1"]);
        let formula = excel
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "formula")
            .unwrap();
        assert_eq!(
            formula.attributes.get("source").map(String::as_str),
            Some("价格!A1")
        );
        assert!(formula
            .attributes
            .get("source")
            .unwrap()
            .find('9')
            .is_none());
        assert!(excel
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "contains"));
        assert!(excel
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "references"));

        let pptx = stored(&[
            ("[Content_Types].xml", "<Types></Types>"),
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                r#"<Relationships><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#,
            ),
            (
                "ppt/presentation.xml",
                r#"<?xml version="1.0"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<?xml version="1.0"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree><p:sp>
    <p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
    <p:txBody><a:p><a:r><a:t>Synthetic</a:t></a:r></a:p></p:txBody>
  </p:sp></p:spTree></p:cSld>
</p:sld>"#,
            ),
            (
                "ppt/notesSlides/notesSlide1.xml",
                r#"<?xml version="1.0"?><p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:t>Speaker</a:t></p:notes>"#,
            ),
        ]);
        let deck = analyze("office/deck.pptx", &pptx);
        assert_eq!(names(&deck, "slide"), vec!["Synthetic"]);
        let slide = deck
            .record
            .entities
            .iter()
            .find(|entity| entity.kind == "slide")
            .unwrap();
        assert_eq!(slide.attributes.get("id").map(String::as_str), Some("256"));
        assert_eq!(
            slide.attributes.get("notes").map(String::as_str),
            Some("Speaker")
        );
        assert!(deck
            .record
            .relationships
            .iter()
            .any(|rel| rel.kind == "contains"));
    }

    #[test]
    fn rejects_empty_truncated_non_zip_and_escaping_targets() {
        for (name, bytes) in [
            ("office/empty.docx", Vec::new()),
            ("office/cut.docx", b"PK\x03\x04truncated".to_vec()),
            ("office/text.docx", b"not a zip".to_vec()),
        ] {
            let result = analyze(name, &bytes);
            assert_eq!(result.status, UnderstandingStatus::Failed, "{name}");
        }
        let escaped = stored(&[(
            "_rels/.rels",
            r#"<Relationships><Relationship Id="rId1" Type="http://example" Target="../secret"/></Relationships>"#,
        )]);
        let result = analyze("office/escape.docx", &escaped);
        assert_eq!(result.status, UnderstandingStatus::Failed);
        assert!(result
            .record
            .diagnostics
            .iter()
            .any(|item| item.message.contains("unsafe path")));
    }

    #[test]
    fn huge_declared_size_is_rejected_before_extract() {
        let mut bytes = stored(&[("word/document.xml", "<w:document/>")]);
        let marker = b"PK\x01\x02";
        let position = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        bytes[position + 24..position + 28].copy_from_slice(&8_000_000u32.to_le_bytes());
        let error = open_opc(&bytes).unwrap_err();
        assert!(matches!(error, OpcError::Limit(ref why) if why == "uncompressed part"));
    }

    #[test]
    fn external_formula_is_kept_with_a_diagnostic() {
        let xlsx = stored(&[
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Id="rId1" Type="officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<Relationships><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Prices" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><c r="B2"><f>'[Other.xlsx]Prices'!A1</f><v>1</v></c></sheetData></worksheet>"#,
            ),
        ]);
        let result = analyze("office/external.xlsx", &xlsx);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        assert_eq!(names(&result, "formula"), vec!["Prices!B2"]);
        assert!(result
            .record
            .diagnostics
            .iter()
            .any(|item| item.message == "unresolved external reference"));
    }

    #[test]
    fn xlsm_does_not_surface_vba() {
        let xlsm = stored(&[
            (
                "_rels/.rels",
                r#"<Relationships><Relationship Id="rId1" Type="officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<workbook><sheets><sheet name="Prices" sheetId="1"/></sheets></workbook>"#,
            ),
            ("xl/vbaProject.bin", "MACROBYTES"),
        ]);
        let result = analyze("office/book.xlsm", &xlsm);
        assert_eq!(result.status, UnderstandingStatus::PartiallyUnderstood);
        let rendered = format!("{result:?}");
        assert!(!rendered.contains("MACROBYTES"));
        assert!(result
            .record
            .diagnostics
            .iter()
            .any(|item| item.message == "macro_not_executed"));
        assert!(result
            .record
            .entities
            .iter()
            .any(|entity| entity.kind == "worksheet"));
    }

    #[test]
    fn encrypted_and_legacy_packages_stay_diagnostics() {
        let mut ole = vec![0xD0, 0xCF, 0x11, 0xE0];
        ole.extend_from_slice(b"legacy-bytes");
        let encrypted = analyze("office/secret.docx", &ole);
        assert_eq!(encrypted.status, UnderstandingStatus::PartiallyUnderstood);
        assert!(encrypted
            .record
            .diagnostics
            .iter()
            .any(|item| item.message == "encrypted"));
        let legacy = analyze("office/legacy.doc", b"not-ole");
        assert!(legacy
            .record
            .diagnostics
            .iter()
            .any(|item| item.message == "unsupported_legacy_format"));
        let marked = stored(&[("EncryptedPackage", "secret")]);
        let package = analyze("office/sealed.xlsx", &marked);
        assert!(package
            .record
            .diagnostics
            .iter()
            .any(|item| item.message == "encrypted"));
    }

    #[test]
    fn opc_facts_include_modified_time() {
        let docx = stored(&[
            ("docProps/core.xml", core()),
            ("docProps/app.xml", app()),
            ("word/document.xml", "<w:document/>"),
        ]);
        let descriptor = object("office/brief.docx");
        let proto = ProtoData {
            filename: "brief.docx".into(),
            extension: Some("docx".into()),
            ..ProtoData::default()
        };
        let evidence = extract_intrinsic(&descriptor, &proto, &docx);
        let title = evidence
            .intrinsic
            .facts
            .iter()
            .find(|fact| fact.key == "opc:title")
            .unwrap();
        assert_eq!(title.value, FactValue::String("Synthetic brief".into()));
        assert!(evidence
            .intrinsic
            .facts
            .iter()
            .any(|fact| fact.key == "opc:creator"));
        assert!(evidence
            .intrinsic
            .facts
            .iter()
            .any(|fact| fact.key == "opc:subject"));
        assert!(evidence
            .intrinsic
            .facts
            .iter()
            .any(|fact| fact.key == "opc:revision"));
        assert!(evidence
            .intrinsic
            .facts
            .iter()
            .any(|fact| fact.key == "opc:application"));
        assert!(evidence
            .intrinsic
            .creation_time()
            .iter()
            .any(|fact| fact.key == "opc:modified"));
        assert!(crate::intrinsic::extractor_stamp().contains("opc:0.1.0"));
    }
}
